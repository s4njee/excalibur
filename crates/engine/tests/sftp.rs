//! The SFTP backend against a real OpenSSH server.
//!
//! A local `sshd` rather than Docker: it is the same software the AC names,
//! it needs no daemon running on the machine, and the alternative — an
//! in-process SFTP server — would be testing a fixture's idea of the protocol
//! rather than OpenSSH's. Skipped unless `sshd` and `sftp-server` are both
//! present, so a machine without them fails nothing.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use excalibur_engine::backend::sftp::{Credential, SftpBackend, SftpTarget};
use excalibur_engine::backend::{CasOutcome, EntryKind, ShareBackend, WriteMode};
use excalibur_engine::hostkeys::{HostKey, KnownHosts};

/// Where OpenSSH keeps its SFTP subsystem, per platform.
fn sftp_server() -> Option<PathBuf> {
    ["/usr/libexec/sftp-server", "/usr/lib/openssh/sftp-server"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn sshd() -> Option<PathBuf> {
    ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// A running `sshd`, its port, and the client key that gets in.
struct Server {
    child: Child,
    port: u16,
    key_path: PathBuf,
    host_key: HostKey,
    root: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start an `sshd` serving `root`, or `None` when the tools are missing.
fn start(root: tempfile::TempDir) -> Option<Server> {
    let (Some(sshd), Some(subsystem)) = (sshd(), sftp_server()) else {
        eprintln!("skipping: needs sshd and sftp-server");
        return None;
    };

    let config_dir = root.path().join(".sshd");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    let host_key = config_dir.join("host_ed25519");
    let client_key = config_dir.join("client_ed25519");
    for (path, kind) in [(&host_key, "ed25519"), (&client_key, "ed25519")] {
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", kind, "-N", "", "-f"])
            .arg(path)
            .status()
            .expect("ssh-keygen");
        assert!(status.success(), "ssh-keygen failed");
    }

    let authorized = config_dir.join("authorized_keys");
    std::fs::copy(client_key.with_extension("pub"), &authorized).expect("authorized_keys");
    // sshd refuses a world- or group-readable authorized_keys.
    permit_owner_only(&authorized);
    permit_owner_only(&host_key);

    // A port the kernel picked and then released. Deriving it from the process
    // id gave every test in the run the same number, so each started an sshd
    // with its own host key on one port and they connected to each other's —
    // which showed up as `HostKeyChanged`, alarmingly and correctly.
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);
        port
    };
    // sshd told to keep to itself: no PAM, no host-based anything, and only
    // the one key that was just made.
    let config = config_dir.join("sshd_config");
    std::fs::write(
        &config,
        format!(
            "Port {port}\n\
             ListenAddress 127.0.0.1\n\
             HostKey {}\n\
             AuthorizedKeysFile {}\n\
             PubkeyAuthentication yes\n\
             PasswordAuthentication no\n\
             KbdInteractiveAuthentication no\n\
             UsePAM no\n\
             StrictModes no\n\
             PidFile none\n\
             Subsystem sftp {}\n",
            host_key.display(),
            authorized.display(),
            subsystem.display(),
        ),
    )
    .expect("sshd_config");

    let child = Command::new(sshd)
        .arg("-D")
        .arg("-f")
        .arg(&config)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    // The host key as `known_hosts` records it, so the trust store can be
    // seeded without a prompt.
    let public = std::fs::read_to_string(host_key.with_extension("pub")).expect("host pub");
    let mut fields = public.split_whitespace();
    let algorithm = fields.next().expect("algorithm").to_owned();
    let blob = fields.next().expect("key").to_owned();

    let server = Server {
        child,
        port,
        key_path: client_key,
        host_key: HostKey {
            host: "127.0.0.1".to_owned(),
            port,
            algorithm,
            key: blob,
        },
        root,
    };
    // sshd takes a moment to bind. Polling beats a fixed sleep.
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(server);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("skipping: sshd did not come up on {port}");
    None
}

fn permit_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Connect to a started server, with its host key already trusted.
async fn connect(server: &Server) -> (SftpBackend, tempfile::TempDir) {
    let store_dir = tempfile::tempdir().expect("store dir");
    let known = KnownHosts::new(store_dir.path().join("known_hosts"));
    known.trust(&server.host_key).expect("trust the host key");

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_owned());
    let root = server.root.path().to_string_lossy().into_owned();
    let target = SftpTarget {
        host: "127.0.0.1",
        port: server.port,
        username: &user,
        root: &root,
        read_only: false,
    };
    let credential = Credential::PrivateKey {
        path: server.key_path.to_string_lossy().into_owned(),
        passphrase: None,
    };
    let backend = SftpBackend::connect(&target, &credential, Arc::new(known))
        .await
        .expect("connect over sftp");
    (backend, store_dir)
}

#[tokio::test]
async fn lists_reads_and_writes_over_a_real_openssh_server() {
    let root = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir(root.path().join("Series")).expect("dir");
    std::fs::write(root.path().join("notes.txt"), b"hello from sftp").expect("file");
    let Some(server) = start(root) else { return };
    let (backend, _store) = connect(&server).await;

    let listing = backend.list_dir("").await.expect("list");
    // The `.sshd` config directory this fixture made is in there too.
    let names: Vec<String> = listing.entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"notes.txt".to_owned()), "{names:?}");
    assert!(names.contains(&"Series".to_owned()), "{names:?}");
    assert_eq!(listing.mount.protocol_label, "SFTP");
    assert!(listing.mount.writable, "{}", listing.mount.reason);

    let folder = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Series")
        .expect("the folder");
    assert_eq!(folder.attrs.kind, EntryKind::Dir);
    assert_eq!(folder.kind_label, "Folder");

    assert_eq!(
        backend
            .read_range("notes.txt", 0, 4096)
            .await
            .expect("read"),
        b"hello from sftp"
    );
    // A ranged read must land at the offset, not at the start.
    assert_eq!(
        backend.read_range("notes.txt", 6, 4).await.expect("ranged"),
        b"from"
    );

    backend
        .write_range("fresh.txt", WriteMode::Create, b"written by excalibur")
        .await
        .expect("write");
    assert_eq!(
        std::fs::read(server.root.path().join("fresh.txt")).expect("read back"),
        b"written by excalibur"
    );

    let capacity = backend.capacity().await.expect("statvfs@openssh.com");
    assert!(capacity.total > 0 && capacity.free <= capacity.total);
}

#[tokio::test]
async fn a_resumed_write_lands_at_its_offset() {
    // APPEND would put it at whatever the length happens to be, which is the
    // same thing only when nothing else has touched the file.
    let root = tempfile::tempdir().expect("temp dir");
    let Some(server) = start(root) else { return };
    let (backend, _store) = connect(&server).await;

    backend
        .write_range("part.bin", WriteMode::Create, b"AAAA")
        .await
        .expect("first");
    backend
        .write_range("part.bin", WriteMode::Resume { offset: 2 }, b"BB")
        .await
        .expect("resume");
    assert_eq!(
        std::fs::read(server.root.path().join("part.bin")).expect("read"),
        b"AABB"
    );
}

#[tokio::test]
async fn the_mutations_work_and_a_replace_never_loses_the_destination() {
    let root = tempfile::tempdir().expect("temp dir");
    let Some(server) = start(root) else { return };
    let (backend, _store) = connect(&server).await;

    // OpenSSH advertises `posix-rename@openssh.com`, so the replace below must
    // take the one-request path rather than the stash-aside fallback. Without
    // this the fallback would pass the same assertions and hide a regression.
    assert!(
        backend.caps().atomic_rename,
        "OpenSSH advertises posix-rename; the extension map was not read"
    );

    backend.mkdir("box").await.expect("mkdir");
    assert!(server.root.path().join("box").is_dir());

    backend
        .write_range("box/one.txt", WriteMode::Create, b"one")
        .await
        .expect("write");
    backend
        .write_range("box/two.txt", WriteMode::Create, b"two")
        .await
        .expect("write");

    // Without `replace`, SFTPv3's own rename refuses an existing destination.
    assert!(backend
        .rename("box/one.txt", "box/two.txt", false)
        .await
        .is_err());

    // With it, the destination is replaced and — the part that matters — it is
    // never absent. The obvious fallback (remove, then rename) destroys the
    // file it is replacing if it dies in between.
    backend
        .rename("box/one.txt", "box/two.txt", true)
        .await
        .expect("replacing rename");
    assert_eq!(
        std::fs::read(server.root.path().join("box/two.txt")).expect("read"),
        b"one"
    );
    assert!(!server.root.path().join("box/one.txt").exists());
    // And nothing was left standing aside.
    assert!(!server
        .root
        .path()
        .join("box/two.txt.excalibur-replacing")
        .exists());

    assert!(
        backend.remove("box", false).await.is_err(),
        "rmdir on a full directory"
    );
    backend.remove("box", true).await.expect("recursive remove");
    assert!(!server.root.path().join("box").exists());
}

#[tokio::test]
async fn a_write_back_refuses_a_file_that_changed_underneath() {
    // SFTP has no change cookie, so this is the portable size-and-mtime
    // comparison. Weaker than xsync's, and it still catches the case the
    // edit-in-place round trip exists for.
    let root = tempfile::tempdir().expect("temp dir");
    let Some(server) = start(root) else { return };
    let (backend, _store) = connect(&server).await;

    backend
        .write_range("doc.txt", WriteMode::Create, b"original")
        .await
        .expect("seed");
    let before = backend.stat("doc.txt").await.expect("stat");
    assert!(
        before.change_cookie.is_none(),
        "SFTP has no cookie to offer"
    );

    let written = backend
        .write_all_if_unchanged("doc.txt", Some(&before), b"my edit")
        .await
        .expect("guarded write");
    assert!(matches!(written, CasOutcome::Written(_)), "{written:?}");

    // Somebody else edits it. A second apart, because the comparison falls
    // back to mtime and SFTPv3 carries it in whole seconds.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(server.root.path().join("doc.txt"), b"somebody else").expect("edit");

    let refused = backend
        .write_all_if_unchanged("doc.txt", Some(&before), b"mine again")
        .await
        .expect("the guard resolves rather than failing");
    assert!(matches!(refused, CasOutcome::Changed(_)), "{refused:?}");
    assert_eq!(
        std::fs::read(server.root.path().join("doc.txt")).expect("read"),
        b"somebody else"
    );
}

#[tokio::test]
async fn an_untrusted_host_key_is_refused_with_its_fingerprint() {
    // The same typed error E3-S4 defined, so the window shows a trust prompt
    // rather than a generic failure — and it comes from the key of the
    // connection actually being made, not from a separate scan.
    let root = tempfile::tempdir().expect("temp dir");
    let Some(server) = start(root) else { return };
    let store_dir = tempfile::tempdir().expect("store dir");
    let known = KnownHosts::new(store_dir.path().join("known_hosts"));

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_owned());
    let root_path = server.root.path().to_string_lossy().into_owned();
    let refused = SftpBackend::connect(
        &SftpTarget {
            host: "127.0.0.1",
            port: server.port,
            username: &user,
            root: &root_path,
            read_only: false,
        },
        &Credential::PrivateKey {
            path: server.key_path.to_string_lossy().into_owned(),
            passphrase: None,
        },
        Arc::new(known),
    )
    .await;

    match refused.map(|_| ()) {
        Err(excalibur_engine::EngineError::HostKeyUnknown { fingerprint, .. }) => {
            assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
        }
        other => panic!("expected HostKeyUnknown, got {other:?}"),
    }
}

#[tokio::test]
async fn a_read_only_mount_refuses_writes_and_says_why() {
    let root = tempfile::tempdir().expect("temp dir");
    let Some(server) = start(root) else { return };
    let store_dir = tempfile::tempdir().expect("store dir");
    let known = KnownHosts::new(store_dir.path().join("known_hosts"));
    known.trust(&server.host_key).expect("trust");

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_owned());
    let root_path = server.root.path().to_string_lossy().into_owned();
    let backend = SftpBackend::connect(
        &SftpTarget {
            host: "127.0.0.1",
            port: server.port,
            username: &user,
            root: &root_path,
            read_only: true,
        },
        &Credential::PrivateKey {
            path: server.key_path.to_string_lossy().into_owned(),
            passphrase: None,
        },
        Arc::new(known),
    )
    .await
    .expect("connect");

    let info = backend.mount_info();
    assert!(!info.writable);
    // The status bar shows this verbatim, so an empty reason would leave the
    // user with a disabled button and no explanation.
    assert!(!info.reason.is_empty(), "{info:?}");

    let refused = backend
        .write_range("blocked.txt", WriteMode::Create, b"nope")
        .await
        .expect_err("a write must be refused");
    assert!(matches!(
        refused,
        excalibur_engine::EngineError::ReadOnly(_)
    ));
    assert!(!server.root.path().join("blocked.txt").exists());
}

#[tokio::test]
async fn a_large_read_is_reassembled_in_order() {
    // The reason for a test this size: reads are split into 256 KiB requests
    // with eight in flight, so they complete out of order. A version that
    // concatenated them in completion order rather than by offset passes every
    // test whose file fits in one chunk, and corrupts every file that does not.

    /// 2.5 MiB — ten chunks, so the pipeline refills several times.
    const SIZE: u32 = 2_621_440;
    /// A range starting and ending inside different chunks, which is where an
    /// off-by-one in the offset arithmetic shows up.
    const OFFSET: u32 = 300_000;
    const LENGTH: u32 = 700_000;

    let root = tempfile::tempdir().expect("temp dir");
    let payload: Vec<u8> = (0..SIZE).map(|index| (index % 251) as u8).collect();
    std::fs::write(root.path().join("big.bin"), &payload).expect("seed");
    let Some(server) = start(root) else { return };
    let (backend, _store) = connect(&server).await;

    let whole = backend
        .read_range("big.bin", 0, SIZE)
        .await
        .expect("read the whole file");
    assert_eq!(whole.len(), payload.len());
    assert!(
        whole == payload,
        "the chunks were reassembled in the wrong order"
    );

    let middle = backend
        .read_range("big.bin", u64::from(OFFSET), LENGTH)
        .await
        .expect("ranged read");
    assert!(
        middle == payload[OFFSET as usize..(OFFSET + LENGTH) as usize],
        "a ranged read across chunk boundaries came back wrong"
    );

    // Asking for more than is there returns what is there, not an error and
    // not padding — the stream protocol relies on a short read meaning EOF.
    let past_the_end = backend
        .read_range("big.bin", u64::from(SIZE) - 10, 4096)
        .await
        .expect("read past the end");
    assert_eq!(past_the_end, payload[payload.len() - 10..]);

    assert!(backend
        .read_range("big.bin", u64::from(SIZE) + 1000, 4096)
        .await
        .expect("read wholly past the end")
        .is_empty());
}

#[tokio::test]
async fn a_large_write_survives_being_split() {
    // Writes are chunked too, at whatever `limits@openssh.com` allows. The
    // failure this catches is a chunk landing at the wrong offset, which
    // produces a file of the right length and the wrong contents.
    let root = tempfile::tempdir().expect("temp dir");
    let Some(server) = start(root) else { return };
    let (backend, _store) = connect(&server).await;

    let payload: Vec<u8> = (0..1_500_000_u32)
        .map(|index| (index % 253) as u8)
        .collect();
    let written = backend
        .write_range("big.bin", WriteMode::Create, &payload)
        .await
        .expect("write");
    assert_eq!(written, u64::try_from(payload.len()).expect("length fits"));
    let on_disk = std::fs::read(server.root.path().join("big.bin")).expect("read back");
    assert!(on_disk == payload, "a chunked write landed out of order");
}

#[tokio::test]
async fn a_directory_larger_than_one_reply_lists_completely() {
    // A server answers `readdir` with as many names as fit in a packet and
    // expects to be asked again. Stopping after the first reply silently
    // truncates the listing, which looks like missing files rather than a bug.
    let root = tempfile::tempdir().expect("temp dir");
    let many = root.path().join("many");
    std::fs::create_dir(&many).expect("dir");
    for index in 0..500 {
        std::fs::write(many.join(format!("file-{index:04}.txt")), b"x").expect("file");
    }
    let Some(server) = start(root) else { return };
    let (backend, _store) = connect(&server).await;

    let listing = backend.list_dir("many").await.expect("list");
    assert_eq!(listing.entries.len(), 500, "the listing was truncated");
}

#[tokio::test]
async fn a_rejected_password_fails_rather_than_hanging() {
    // The password credential tries `password` and then falls back to
    // `keyboard-interactive`. This fixture's sshd offers neither, so the
    // fallback runs out — which is the case where a loop waiting for prompts
    // that never come would hang the mount instead of reporting a refusal.
    // The accepting path needs a real account password, so it is not testable
    // here; this covers the half that can go wrong silently.
    let root = tempfile::tempdir().expect("temp dir");
    let Some(server) = start(root) else { return };
    let store_dir = tempfile::tempdir().expect("store dir");
    let known = KnownHosts::new(store_dir.path().join("known_hosts"));
    known.trust(&server.host_key).expect("trust");

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_owned());
    let root_path = server.root.path().to_string_lossy().into_owned();
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        SftpBackend::connect(
            &SftpTarget {
                host: "127.0.0.1",
                port: server.port,
                username: &user,
                root: &root_path,
                read_only: false,
            },
            &Credential::Password("not the password".to_owned()),
            Arc::new(known),
        ),
    )
    .await
    .expect("authentication must not hang");

    assert!(matches!(
        refused.map(|_| ()),
        Err(excalibur_engine::EngineError::AuthenticationFailed(_))
    ));
}
