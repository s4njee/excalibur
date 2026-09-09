//! Local↔xsync transfers through xsync's engine, against a real `xs --server`
//! (E22-S1).
//!
//! Skipped unless `EXCALIBUR_XS` names an `xs` binary, for the same reason
//! `xsync.rs` is: the alternative is either a mock of the protocol — which
//! would test the mock — or a hard dependency on a sibling checkout being
//! built, which would fail the suite for anyone who has not built it.
//!
//! What is under test is the *queue*, not the adapter in isolation. A row goes
//! in, the worker picks it up, the routing rule sends it through
//! `run_client_push`/`run_client_pull`, and the assertions are the three things
//! a person can see: the bytes on the far side, what the row says, and what a
//! cancel leaves behind.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::xsync::XsyncBackend;
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::xsync_engine::{XsyncRemote, XsyncRoute};
use excalibur_engine::{Direction, Result, ShareBackend, Transfer, TransferStatus};

/// The `xs` binary to serve with, or `None` to skip.
fn xs_binary() -> Option<String> {
    let path = std::env::var("EXCALIBUR_XS").ok()?;
    PathBuf::from(&path).is_file().then_some(path)
}

/// A mount that is one local `xs --server`, and knows it is one.
///
/// The two answers are deliberately different objects: `active` is the live v3
/// browse session, and `xsync_route` is the recipe for the *second* process a
/// transfer spawns. That is the shape the Tauri shell has too.
struct Export {
    backend: Arc<dyn ShareBackend>,
    route: XsyncRoute,
}

#[async_trait]
impl BackendSource for Export {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        Ok(Arc::clone(&self.backend))
    }

    async fn xsync_route(&self, _mount: Option<&str>) -> Option<XsyncRoute> {
        Some(self.route.clone())
    }
}

async fn export(xs: &str, root: &Path, writable: bool) -> Arc<Export> {
    let backend = XsyncBackend::connect_local(xs, root.to_str().unwrap(), !writable, None)
        .await
        .expect("mount the export");
    Arc::new(Export {
        backend: Arc::new(backend),
        route: XsyncRoute {
            remote: XsyncRemote::Local {
                program: xs.to_owned(),
            },
            export_root: root.to_string_lossy().into_owned(),
            writable,
            reason: if writable {
                String::new()
            } else {
                "export is read-only".to_owned()
            },
        },
    })
}

/// Run one row to completion and hand back what the row ended up saying.
async fn run(source: Arc<Export>, transfer: Transfer) -> Transfer {
    let queue = Arc::new(TransferQueue::in_memory());
    let id = transfer.id.clone();
    queue.add(transfer).await;
    let worker = Arc::new(Worker::new(Arc::clone(&queue), source, 1));
    worker.drain().await;
    queue
        .all()
        .await
        .into_iter()
        .find(|item| item.id == id)
        .expect("the row survived")
}

/// A tree with the three shapes E22-S1 asks about: a file large enough to take
/// the chunked path, a small one, a **symlink**, and an **empty directory**.
fn populate(root: &Path) -> BTreeMap<String, [u8; 32]> {
    fs::create_dir_all(root.join("nested/deep")).unwrap();
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::write(
        root.join("README"),
        b"a small file, sent in the batched pass\n",
    )
    .unwrap();
    fs::write(root.join("nested/notes.txt"), vec![b'n'; 4096]).unwrap();
    // Over the single-segment limit, so this one is chunked and is the only
    // file in the tree that produces `Progress` events at all.
    fs::write(root.join("nested/deep/blob.bin"), incompressible(9 << 20)).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("nested/notes.txt", root.join("shortcut")).unwrap();
    digests(root)
}

/// Bytes zstd cannot shrink, so a wire figure is a real payload rather than a
/// run-length trick.
fn incompressible(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// `path -> blake3` for every regular file under `root`, plus a marker for each
/// directory and symlink so the assertion covers the shapes that carry no
/// bytes. An empty directory that silently did not arrive is exactly the kind
/// of omission a digest-only comparison cannot see.
fn digests(root: &Path) -> BTreeMap<String, [u8; 32]> {
    let mut found = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // `symlink_metadata`, so a link is a link and not the file it names.
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_symlink() {
                let target = fs::read_link(&path).unwrap();
                found.insert(
                    format!("{relative} -> link"),
                    *blake3::hash(target.to_string_lossy().as_bytes()).as_bytes(),
                );
            } else if kind.is_dir() {
                found.insert(format!("{relative}/"), [0; 32]);
                stack.push(path);
            } else {
                found.insert(
                    relative,
                    *blake3::hash(&fs::read(&path).unwrap()).as_bytes(),
                );
            }
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tree_pushes_and_pulls_through_the_queue() {
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("source");
    let share = workspace.path().join("share");
    let back = workspace.path().join("back");
    let expected = populate(&source);
    fs::create_dir_all(&share).unwrap();
    fs::create_dir_all(&back).unwrap();

    // Push: the whole tree is *one row*. The planner does the walk, so nothing
    // on this side stats a file before the session starts.
    let mount = export(&xs, &share, true).await;
    let pushed = run(
        Arc::clone(&mount),
        Transfer::new("push", Direction::Up, source.to_string_lossy(), "landed", 0),
    )
    .await;

    assert_eq!(pushed.status, TransferStatus::Done, "{:?}", pushed.error);
    assert!(pushed.verified, "a push through xsync's engine is verified");
    assert_eq!(
        digests(&share.join("landed")),
        expected,
        "push published the wrong tree"
    );
    assert!(pushed.bytes_done > 9 << 20, "bytes: {}", pushed.bytes_done);
    // `Planned` is what fills this in: the row was enqueued not knowing.
    assert!(pushed.bytes_total > 0, "the planner never sized the row");

    // Pull the same tree back.
    let pulled = run(
        Arc::clone(&mount),
        Transfer::new(
            "pull",
            Direction::Down,
            "landed",
            back.join("landed").to_string_lossy(),
            0,
        ),
    )
    .await;

    assert_eq!(pulled.status, TransferStatus::Done, "{:?}", pulled.error);
    assert!(pulled.verified);
    assert_eq!(
        digests(&back.join("landed")),
        expected,
        "pull published the wrong tree"
    );
}

/// The counts assertion that matters most: the difference between "the transfer
/// worked" and "the transfer ran".
#[tokio::test(flavor = "multi_thread")]
async fn a_second_push_moves_nothing_and_says_so() {
    let Some(xs) = xs_binary() else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("source");
    let share = workspace.path().join("share");
    populate(&source);
    fs::create_dir_all(&share).unwrap();
    let mount = export(&xs, &share, true).await;

    let first = run(
        Arc::clone(&mount),
        Transfer::new("a", Direction::Up, source.to_string_lossy(), "landed", 0),
    )
    .await;
    assert_eq!(first.status, TransferStatus::Done, "{:?}", first.error);
    assert!(first.note.is_none(), "the first push moved the bytes");

    let second = run(
        Arc::clone(&mount),
        Transfer::new("b", Direction::Up, source.to_string_lossy(), "landed", 0),
    )
    .await;
    assert_eq!(second.status, TransferStatus::Done, "{:?}", second.error);
    // Nothing was published, so there is nothing to vouch for.
    assert!(
        !second.verified,
        "a run that moved no bytes verified nothing"
    );
    assert!(
        second.note.is_some_and(|note| note.contains("unchanged")),
        "the row did not say the tree was already there"
    );
}

/// A single file goes to the name the row promised, not to the source's.
#[tokio::test(flavor = "multi_thread")]
async fn keep_both_lands_a_file_under_its_new_name() {
    let Some(xs) = xs_binary() else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("report.pdf");
    fs::write(&source, b"the second one").unwrap();
    let share = workspace.path().join("share");
    fs::create_dir_all(&share).unwrap();
    // Something already in the way, which is what makes this Keep both.
    fs::write(share.join("report.pdf"), b"the first one").unwrap();

    let mount = export(&xs, &share, true).await;
    let row = run(
        Arc::clone(&mount),
        // `conflict::keep_both_name` produced this name; the engine cannot
        // rename a single file, so the adapter stages a hard link under it.
        Transfer::new(
            "keep",
            Direction::Up,
            source.to_string_lossy(),
            "report 2.pdf",
            0,
        ),
    )
    .await;

    assert_eq!(row.status, TransferStatus::Done, "{:?}", row.error);
    assert_eq!(
        fs::read(share.join("report 2.pdf")).unwrap(),
        b"the second one"
    );
    // The whole point: what was already there is untouched.
    assert_eq!(
        fs::read(share.join("report.pdf")).unwrap(),
        b"the first one"
    );
    // And the scaffolding is gone from beside the source.
    let beside: Vec<_> = fs::read_dir(workspace.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".excalibur-"))
        .collect();
    assert!(
        beside.is_empty(),
        "staging outlived the transfer: {beside:?}"
    );
}

/// Pulling one file lands the bytes, whichever route carries it.
///
/// It is the chunk loop today, and deliberately: `xs --server` cannot serve a
/// *file* as the source of a pull — see `Worker::attempt`. The assertion here
/// is about the outcome rather than the route, so it keeps holding when
/// E22-S2 sends this through xsync's engine too.
#[tokio::test(flavor = "multi_thread")]
async fn pulling_one_file_lands_it() {
    let Some(xs) = xs_binary() else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let share = workspace.path().join("share");
    fs::create_dir_all(share.join("nested")).unwrap();
    let payload = incompressible(3 << 20);
    fs::write(share.join("nested/one.bin"), &payload).unwrap();
    let landing = workspace.path().join("here/one.bin");

    let mount = export(&xs, &share, true).await;
    let row = run(
        mount,
        Transfer::new(
            "one",
            Direction::Down,
            "nested/one.bin",
            landing.to_string_lossy(),
            0,
        ),
    )
    .await;

    assert_eq!(row.status, TransferStatus::Done, "{:?}", row.error);
    assert_eq!(
        fs::read(&landing).unwrap(),
        payload,
        "the file did not arrive"
    );
    // The one thing that must never happen: a row that says it finished while
    // the destination is empty. That is what the file-root pull did.
    assert!(
        row.bytes_done > 0,
        "the row claimed success having moved nothing"
    );
}

/// Many small files in one batch do not deadlock the session.
///
/// The protocol acknowledges every frame and the peer blocks once its replies
/// fill the pipe back to us. xsync sizes its unacknowledged window to sit
/// inside OpenSSH's 2 MiB channel window; a plain pipe holds 64 KiB, which is
/// *below* that window, so a client that blocks writing never reads the
/// replies and the two ends wait for each other forever. Before
/// `Detached`, a 10,000-file push into a local `xs --server` stopped dead at
/// about 1,600 files with every process at 0% CPU.
///
/// Three thousand files is past that cliff and still under two seconds.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_of_small_files_does_not_deadlock() {
    let Some(xs) = xs_binary() else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("many");
    let share = workspace.path().join("share");
    fs::create_dir_all(&share).unwrap();
    for bucket in 0..30 {
        let directory = source.join(format!("{bucket:02}"));
        fs::create_dir_all(&directory).unwrap();
        for index in 0..100 {
            fs::write(directory.join(format!("f{index:03}")), vec![b'x'; 8192]).unwrap();
        }
    }

    let mount = export(&xs, &share, true).await;
    let row = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        run(
            mount,
            Transfer::new("many", Direction::Up, source.to_string_lossy(), "many", 0),
        ),
    )
    .await
    .expect("the small-file batch deadlocked");

    assert_eq!(row.status, TransferStatus::Done, "{:?}", row.error);
    let landed = fs::read_dir(share.join("many")).unwrap().count();
    assert_eq!(landed, 30, "only {landed} of 30 directories arrived");

    // And back, because the deadlock is not one-directional: pulling the same
    // tree stalls at the same place for the mirrored reason — the child blocked
    // writing data while we are blocked writing acknowledgements it is not
    // reading. A queue bounded by message count rather than bytes fixes the
    // push and leaves this one hanging.
    let back = workspace.path().join("back");
    let pulled = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        run(
            export(&xs, &share, true).await,
            Transfer::new("back", Direction::Down, "many", back.to_string_lossy(), 0),
        ),
    )
    .await
    .expect("the small-file batch deadlocked on the way back");
    assert_eq!(pulled.status, TransferStatus::Done, "{:?}", pulled.error);
    assert_eq!(fs::read_dir(&back).unwrap().count(), 30);
}

/// A push into a read-only mount is refused before a byte moves.
///
/// This is the only gate there is. `xs --server --read-only` sets the v3
/// mount's access and nothing else — the v1/v2 sink never consults it, so a
/// push to a read-only export still writes. Excalibur's `writable` is what
/// stops it.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_mount_refuses_a_push() {
    let Some(xs) = xs_binary() else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("a.txt");
    fs::write(&source, b"nope").unwrap();
    let share = workspace.path().join("share");
    fs::create_dir_all(&share).unwrap();

    let mount = export(&xs, &share, false).await;
    let row = run(
        mount,
        Transfer::new("ro", Direction::Up, source.to_string_lossy(), "a.txt", 0),
    )
    .await;

    assert_eq!(row.status, TransferStatus::Failed);
    assert!(
        row.error
            .as_deref()
            .is_some_and(|e| e.contains("read-only")),
        "{:?}",
        row.error
    );
    assert!(
        !share.join("a.txt").exists(),
        "a read-only export was written to"
    );
}

/// Cancelling kills the transfer's own `ssh`, and the destination is left
/// without a file under the name the transfer was going to publish.
///
/// The staged bytes are *kept*, under `.xsync.tmp.<hex>`: that is the resume
/// point, and it is the whole difference between this route and the `.part`
/// mechanism, which a cancel deletes. What must never survive is the final
/// name, because a half-written file under it is one nothing can tell from a
/// good one.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_leaves_no_file_under_the_final_name() {
    let Some(xs) = xs_binary() else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("big.bin");
    // Big enough that the chunked path is still running when the cancel lands.
    fs::write(&source, incompressible(192 << 20)).unwrap();
    let share = workspace.path().join("share");
    fs::create_dir_all(&share).unwrap();

    let mount = export(&xs, &share, true).await;
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "big",
            Direction::Up,
            source.to_string_lossy(),
            "big.bin",
            0,
        ))
        .await;
    let worker = Arc::new(Worker::new(Arc::clone(&queue), mount, 1));

    let draining = {
        let worker = Arc::clone(&worker);
        tokio::spawn(async move { worker.drain().await })
    };
    // Let it get past the handshake and into the bytes.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    worker.stop("big", TransferStatus::Cancelled).await;
    // The child is killed within one poll interval; the session then fails
    // with a transport error, which the adapter reads as the user's decision.
    tokio::time::timeout(std::time::Duration::from_secs(20), draining)
        .await
        .expect("the cancel did not stop the transfer")
        .unwrap();

    let row = queue.all().await.into_iter().next().expect("the row");
    assert_eq!(row.status, TransferStatus::Cancelled, "{:?}", row.error);
    assert!(
        !share.join("big.bin").exists(),
        "a cancelled transfer left a file under its final name"
    );
    // Nothing else under the final name either — the stage is a dotfile.
    let published: Vec<String> = fs::read_dir(&share)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with(".xsync.tmp."))
        .collect();
    assert!(
        published.is_empty(),
        "cancelled transfer published {published:?}"
    );
}
