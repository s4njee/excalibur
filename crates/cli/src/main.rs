//! `xcal` — drive the engine without a window.
//!
//! The engine has no Tauri dependency precisely so this can exist: bringing up
//! a protocol backend, or reproducing a listing bug, should not require
//! building a GUI. Today it speaks to the mock; each backend joins as it lands.

use std::process::ExitCode;

use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::xsync::{SshTarget, XsyncBackend};
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::xsync_engine::{SshEndpoint, XsyncRemote, XsyncRoute};
use excalibur_engine::transfer::{plan, Direction, Status, Transfer};
use excalibur_engine::{hostkeys, EntryKind, KnownHosts, Result, ShareBackend, Trust};

/// The one backend this process has, handed to the transfer worker.
///
/// The worker asks for a backend per attempt so a reconnect can hand it a new
/// one. There is no session manager here, so the answer is always the same
/// mount — which is also what makes `get` measure the transfer path and
/// nothing else.
struct Mounted {
    backend: Arc<dyn ShareBackend>,
    /// Set by `--xsync-engine`, and the only thing that routes a transfer
    /// through xsync's own push/pull engine rather than the worker's chunk
    /// loop (E22-S1). Off by default so `get` keeps measuring what
    /// `docs/benchmarks.md` says it measures for E13-S7, and the two figures
    /// stay comparable.
    route: Option<XsyncRoute>,
}

#[async_trait]
impl BackendSource for Mounted {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        Ok(Arc::clone(&self.backend))
    }

    async fn xsync_route(&self, _mount: Option<&str>) -> Option<XsyncRoute> {
        self.route.clone()
    }
}

const USAGE: &str = "\
xcal — headless Excalibur engine driver

USAGE:
    xcal ls [PATH]        list a directory of the mock share
    xcal stat PATH        metadata for one path
    xcal mount            the mount facts a status bar would show
    xcal cat PATH         read a file through the backend
    xcal get REMOTE LOCAL download through the transfer worker
    xcal put LOCAL REMOTE upload through the transfer worker
    xcal write PATH TEXT  write a file through the backend
    xcal df               capacity
    xcal reachable HOST PORT
                          the sidebar's TCP probe, for a host that dims

By default these run against the in-memory mock. To drive a real share
instead:

    --local PATH          browse a folder on this machine (E21)
    --xsync-local PATH    run `xs --server PATH` and speak v3 over its pipes
    --xs BINARY           which xs to run (default: xs on PATH)
    --xsync-ssh HOST:PATH connect over ssh
    --port N              ssh port (default 22)
    --user NAME           ssh username
    --trust               accept an unknown host key and record it
    --depth N             reads in flight per download, for `get` (default 4)
    --xsync-engine        run `get`/`put` through xsync's own push/pull engine
                          instead of the worker's chunk loop (E22-S1). Only
                          meaningful with --xsync-local or --xsync-ssh; it is
                          what the GUI does for a Local<->xsync pair, and it
                          is how that route is measured headlessly.

Every command takes --read-only, and --json to print machine-readable
output.
";

#[tokio::main]
// Argument parsing, backend selection and one arm per command, read top to
// bottom. A CLI this small does not gain from being spread across helpers; it
// grows a proper parser when it grows a proper set of commands.
#[allow(clippy::too_many_lines)]
async fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let json = arguments.iter().any(|argument| argument == "--json");
    let read_only = arguments.iter().any(|argument| argument == "--read-only");
    let trust = arguments.iter().any(|argument| argument == "--trust");
    let through_xsync = arguments
        .iter()
        .any(|argument| argument == "--xsync-engine");
    // A flag's value is not a positional argument, so skip whatever follows
    // the flags that take one.
    let takes_value = [
        "--local",
        "--xsync-local",
        "--xsync-ssh",
        "--xs",
        "--port",
        "--user",
        "--depth",
    ];
    let mut positional: Vec<&String> = Vec::new();
    let mut skip_next = false;
    for argument in &arguments {
        if skip_next {
            skip_next = false;
            continue;
        }
        if argument.starts_with("--") {
            skip_next = takes_value.contains(&argument.as_str());
            continue;
        }
        positional.push(argument);
    }

    let value_of = |flag: &str| -> Option<String> {
        let index = arguments.iter().position(|argument| argument == flag)?;
        arguments.get(index + 1).cloned()
    };

    let backend: Arc<dyn ShareBackend> = if let Some(root) = value_of("--local") {
        // No connection, no credential, no host: a folder either opens or it
        // does not. That is the whole reason E21 is a backend rather than a
        // mode — everything below this line is the same for all five.
        match LocalBackend::open(std::path::Path::new(&root), read_only).await {
            Ok(backend) => Arc::new(backend),
            Err(error) => {
                eprintln!("xcal: could not open {root}: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(root) = value_of("--xsync-local") {
        let program = value_of("--xs").unwrap_or_else(|| "xs".to_owned());
        match XsyncBackend::connect_local(&program, &root, read_only).await {
            Ok(backend) => Arc::new(backend),
            Err(error) => {
                eprintln!("xcal: could not mount {root} over xsync: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(target) = value_of("--xsync-ssh") {
        let Some((host, root)) = target.split_once(':') else {
            eprintln!("xcal: --xsync-ssh wants HOST:PATH");
            return ExitCode::from(2);
        };
        let port = match value_of("--port").map(|value| value.parse::<u16>()) {
            None => 22,
            Some(Ok(port)) => port,
            Some(Err(error)) => {
                eprintln!("xcal: --port: {error}");
                return ExitCode::from(2);
            }
        };
        let username = value_of("--user");
        let known_hosts = match KnownHosts::app_default() {
            Ok(known) => known,
            Err(error) => {
                eprintln!("xcal: {error}");
                return ExitCode::FAILURE;
            }
        };

        // The same verification the window does, before ssh gets a chance to
        // ask a question no terminal here would necessarily answer. Running it
        // in the CLI is also how this path gets exercised against real hosts.
        match hostkeys::verify(&hostkeys::Keyscan, &known_hosts, host, port).await {
            Ok(Trust::Trusted) => {}
            Ok(Trust::Unknown { offered }) => {
                if trust {
                    if let Err(error) = known_hosts.trust(&offered) {
                        eprintln!("xcal: could not record host key: {error}");
                        return ExitCode::FAILURE;
                    }
                    eprintln!(
                        "xcal: trusted {} key {} for {host}:{port}",
                        offered.algorithm,
                        offered.fingerprint()
                    );
                } else {
                    eprintln!(
                        "xcal: {host}:{port} is not a known host.\n  \
                         {} key fingerprint is {}\n  \
                         Check that against the server, then re-run with --trust.",
                        offered.algorithm,
                        offered.fingerprint()
                    );
                    return ExitCode::FAILURE;
                }
            }
            Ok(Trust::Changed { offered, stored }) => {
                eprintln!(
                    "xcal: the {} host key for {host}:{port} has CHANGED.\n  \
                     expected {}\n  \
                     got      {}\n  \
                     Someone may be impersonating the host, or it may have been \
                     rebuilt. --trust will not override this; remove the old key \
                     from {} once you know which it is.",
                    offered.algorithm,
                    stored.fingerprint(),
                    offered.fingerprint(),
                    known_hosts.path().display()
                );
                return ExitCode::FAILURE;
            }
            Ok(Trust::Revoked { offered }) => {
                eprintln!(
                    "xcal: the host key {} for {host}:{port} is marked revoked.",
                    offered.fingerprint()
                );
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("xcal: could not check the host key for {host}:{port}: {error}");
                return ExitCode::FAILURE;
            }
        }

        let target_spec = SshTarget {
            host,
            port,
            username: username.as_deref(),
            known_hosts: known_hosts.path(),
            rsh: None,
        };
        match XsyncBackend::connect_ssh(&target_spec, root).await {
            Ok(backend) => Arc::new(backend),
            Err(error) => {
                eprintln!("xcal: could not mount {target} over xsync: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else if read_only {
        Arc::new(MockBackend::media_share().read_only("export is read-only"))
    } else {
        Arc::new(MockBackend::media_share())
    };

    // The recipe for the *second* process a transfer through xsync's engine
    // spawns (E22-S1). Rebuilt from the flags rather than threaded out of the
    // branch above, because it is a different object from the mount: one
    // `xs --server` serves one session, and a browse session and a sync
    // session are different protocols chosen at the handshake.
    let route: Option<XsyncRoute> = if !through_xsync {
        None
    } else if let Some(root) = value_of("--xsync-local") {
        Some(XsyncRoute {
            remote: XsyncRemote::Local {
                program: value_of("--xs").unwrap_or_else(|| "xs".to_owned()),
            },
            export_root: root,
            // The local `xs --server` case: `--read-only` is honoured here
            // because Excalibur's gate is the only one there is. The flag sets
            // the v3 mount's access and nothing else — the v1/v2 sink never
            // consults it, so a push to a read-only export still writes.
            writable: !read_only,
            reason: "export is read-only".to_owned(),
        })
    } else if let Some(target) = value_of("--xsync-ssh") {
        let (host, root) = target.split_once(':').unwrap_or((target.as_str(), ""));
        let known_hosts = match KnownHosts::app_default() {
            Ok(known) => known,
            Err(error) => {
                eprintln!("xcal: {error}");
                return ExitCode::FAILURE;
            }
        };
        Some(XsyncRoute {
            remote: XsyncRemote::Ssh(SshEndpoint {
                host: host.to_owned(),
                port: value_of("--port")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(22),
                username: value_of("--user"),
                // The same file the mount above verified against, so the
                // transfer's own `ssh` enforces the same decision.
                known_hosts: known_hosts.path().to_path_buf(),
                rsh: None,
            }),
            export_root: root.to_owned(),
            writable: !read_only,
            reason: "export is read-only".to_owned(),
        })
    } else {
        eprintln!("xcal: --xsync-engine needs --xsync-local or --xsync-ssh");
        return ExitCode::from(2);
    };

    let Some(command) = positional.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };

    let outcome = match command.as_str() {
        "ls" => {
            let path = positional.get(1).map_or("", |path| path.as_str());
            match backend.list_dir(path).await {
                Ok(listing) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&listing).expect("listing serialises")
                        );
                    } else {
                        for entry in &listing.entries {
                            println!(
                                "{:<28} {:>14} {}",
                                entry.name, entry.attrs.size, entry.kind_label
                            );
                        }
                        let mount = &listing.mount;
                        println!(
                            "\n{} · {} items · {}",
                            if mount.writable { "RW" } else { "RO" },
                            listing.entries.len(),
                            if mount.writable {
                                "read & write".to_owned()
                            } else {
                                mount.reason.clone()
                            }
                        );
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        "stat" => {
            let Some(path) = positional.get(1) else {
                eprintln!("stat needs a path");
                return ExitCode::from(2);
            };
            backend.stat(path).await.map(|attrs| {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&attrs).expect("attrs serialise")
                );
            })
        }
        "mount" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&backend.mount_info()).expect("mount serialises")
            );
            Ok(())
        }
        "cat" => {
            let Some(path) = positional.get(1) else {
                eprintln!("cat needs a path");
                return ExitCode::from(2);
            };
            // Reads go through the backend's verified path where the protocol
            // has one, so this also exercises the digest check.
            backend.read_range(path, 0, 64 * 1024).await.map(|bytes| {
                use std::io::Write as _;
                let _ = std::io::stdout().write_all(&bytes);
            })
        }
        "get" => {
            let (Some(remote), Some(local)) = (positional.get(1), positional.get(2)) else {
                eprintln!("get needs a remote path and a local one");
                return ExitCode::from(2);
            };
            let depth = match value_of("--depth").map(|value| value.parse::<usize>()) {
                None => None,
                Some(Ok(depth)) => Some(depth),
                Some(Err(error)) => {
                    eprintln!("xcal: --depth: {error}");
                    return ExitCode::from(2);
                }
            };
            get(&backend, route, remote, local, depth, json).await
        }
        "put" => {
            let (Some(local), Some(remote)) = (positional.get(1), positional.get(2)) else {
                eprintln!("put needs a local path and a remote one");
                return ExitCode::from(2);
            };
            put(&backend, route, local, remote, json).await
        }
        "write" => {
            let (Some(path), Some(text)) = (positional.get(1), positional.get(2)) else {
                eprintln!("write needs a path and some text");
                return ExitCode::from(2);
            };
            backend
                .write_range(path, excalibur_engine::WriteMode::Create, text.as_bytes())
                .await
                .map(|count| println!("wrote {count} bytes"))
        }
        "reachable" => {
            let (Some(host), Some(port)) = (positional.get(1), positional.get(2)) else {
                eprintln!("reachable needs a host and a port");
                return ExitCode::from(2);
            };
            let Ok(port) = port.parse::<u16>() else {
                eprintln!("{port} is not a port");
                return ExitCode::from(2);
            };
            let started = std::time::Instant::now();
            let up =
                excalibur_engine::session::reachable(host, port, std::time::Duration::from_secs(2))
                    .await;
            println!(
                "{host}:{port} {} in {} ms",
                if up { "reachable" } else { "unreachable" },
                started.elapsed().as_millis()
            );
            return if up {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            };
        }
        "df" => backend.capacity().await.map(|capacity| {
            println!("{} free of {}", capacity.free, capacity.total);
        }),
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xcal: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `xcal get REMOTE LOCAL` — a download through the real transfer worker.
///
/// The point of this command is measurement: the GUI's throughput is the
/// worker's throughput, and a number taken through a window is a number taken
/// through a window. Everything here — the queue, the worker, the streaming
/// read path, the `.part` file and its fsync — is what the app itself runs, so
/// `docs/benchmarks.md` can quote a figure that means something.
///
/// A directory is expanded by the same walk the Download verb uses (E13-S4),
/// which is also the only way to exercise that walk against a real server
/// without a mouse.
async fn get(
    backend: &Arc<dyn ShareBackend>,
    route: Option<XsyncRoute>,
    remote: &str,
    local: &str,
    depth: Option<usize>,
    json: bool,
) -> Result<()> {
    let queue = Arc::new(TransferQueue::in_memory());

    // Through xsync's engine the selection *is* the plan: one row for what was
    // asked for, and xsync's planner does the walk. Pre-walking here would
    // stat every file over the browse session before the transfer session had
    // sent a byte, and then the planner would walk it again.
    if route.is_some() {
        queue
            .add(Transfer::new(
                "one",
                Direction::Down,
                remote.to_owned(),
                local.to_owned(),
                0,
            ))
            .await;
        return drain(backend, route, queue, depth, json).await;
    }

    let attrs = backend.stat(remote).await?;
    if attrs.kind == EntryKind::Dir {
        // `LOCAL` is the folder the copy lands *in*, as with `cp -r`.
        let plan = plan::plan_downloads(
            backend.as_ref(),
            &[remote.to_owned()],
            std::path::Path::new(local),
        )
        .await;
        for problem in &plan.problems {
            eprintln!("xcal: {}: {}", problem.path, problem.reason);
        }
        for directory in &plan.directories {
            if let Err(error) = std::fs::create_dir_all(directory) {
                eprintln!("xcal: create {}: {error}", directory.display());
            }
        }
        for (index, file) in plan.files.iter().enumerate() {
            queue
                .add(Transfer::new(
                    format!("f{index}"),
                    Direction::Down,
                    file.source.clone(),
                    file.destination.to_string_lossy().into_owned(),
                    file.size,
                ))
                .await;
        }
    } else {
        // A single file goes exactly where it was told, rather than into a
        // directory of that name: `get big.bin /tmp/big.bin` should write
        // `/tmp/big.bin`.
        queue
            .add(Transfer::new(
                "one",
                Direction::Down,
                remote.to_owned(),
                local.to_owned(),
                attrs.size,
            ))
            .await;
    }

    drain(backend, route, queue, depth, json).await
}

/// `xcal put LOCAL REMOTE` — an upload through the real transfer worker.
///
/// The counterpart `get` never had, and the reason it exists now is
/// measurement: E22-S1's route moves bytes in both directions and there was no
/// headless way to time the one going out. Without `--xsync-engine` this is one
/// file through the worker's chunk loop, because that path has no directory
/// walk for uploads; with it, a whole tree is one row and xsync plans it.
async fn put(
    backend: &Arc<dyn ShareBackend>,
    route: Option<XsyncRoute>,
    local: &str,
    remote: &str,
    json: bool,
) -> Result<()> {
    let path = std::path::Path::new(local);
    let size = std::fs::metadata(path).map_or(0, |meta| meta.len());
    if route.is_none() && path.is_dir() {
        return Err(excalibur_engine::EngineError::Io(format!(
            "{local} is a directory; uploading one needs --xsync-engine"
        )));
    }
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "one",
            Direction::Up,
            local.to_owned(),
            remote.to_owned(),
            size,
        ))
        .await;
    drain(backend, route, queue, None, json).await
}

/// Run a queue to completion and print what it moved and how fast.
///
/// Shared by `get` and `put` so the two report the same numbers the same way,
/// and so `docs/benchmarks.md` can quote either without a footnote about which
/// timer was running.
async fn drain(
    backend: &Arc<dyn ShareBackend>,
    route: Option<XsyncRoute>,
    queue: Arc<TransferQueue>,
    depth: Option<usize>,
    json: bool,
) -> Result<()> {
    let engine = route.is_some();
    let worker = Arc::new(Worker::new(
        Arc::clone(&queue),
        Arc::new(Mounted {
            backend: Arc::clone(backend),
            route,
        }),
        3,
    ));
    if let Some(depth) = depth {
        worker.set_read_depth(depth);
    }

    let started = std::time::Instant::now();
    worker.drain().await;
    let elapsed = started.elapsed();

    let done = queue.all().await;
    let moved: u64 = done.iter().map(|transfer| transfer.bytes_done).sum();
    let failed: Vec<&Transfer> = done
        .iter()
        .filter(|transfer| transfer.status != Status::Done)
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let rate = moved as f64 / elapsed.as_secs_f64() / 1_000_000.0;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "bytes": moved,
                "seconds": elapsed.as_secs_f64(),
                "megabytesPerSecond": rate,
                "files": done.len(),
                "failed": failed.len(),
                "depth": worker.read_depth(),
                "engine": if engine { "xsync" } else { "worker" },
                "verified": done.iter().all(|transfer| transfer.verified),
            })
        );
    } else if engine {
        // A row that carries a note moved nothing — the planner found the file
        // already there. Its bytes still count as accounted for, which is what
        // fills the progress bar, but dividing them by the elapsed time
        // produces a throughput figure for a transfer that did not happen.
        let notes: Vec<&str> = done
            .iter()
            .filter_map(|transfer| transfer.note.as_deref())
            .collect();
        let speed = if notes.len() == done.len() {
            format!("nothing moved — {}", notes.join(", "))
        } else {
            format!("{rate:.1} MB/s")
        };
        // Depth is a property of the chunk loop and means nothing here; saying
        // it anyway would invite a comparison between two numbers that were
        // not measuring the same thing.
        println!(
            "{moved} bytes in {:.2}s — {speed} ({} row(s), xsync engine{})",
            elapsed.as_secs_f64(),
            done.len(),
            if done.iter().all(|transfer| transfer.verified) {
                ", verified"
            } else {
                ""
            }
        );
    } else {
        println!(
            "{moved} bytes in {:.2}s — {rate:.1} MB/s ({} file(s), depth {})",
            elapsed.as_secs_f64(),
            done.len(),
            worker.read_depth()
        );
    }
    for transfer in &failed {
        eprintln!(
            "xcal: {} failed: {}",
            transfer.source_path,
            transfer.error.as_deref().unwrap_or("stopped")
        );
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(excalibur_engine::EngineError::Io(format!(
            "{} of {} transfers did not finish",
            failed.len(),
            done.len()
        )))
    }
}
