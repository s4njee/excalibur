//! What a user is told when `ssh` fails before the session starts (E7-S4).
//!
//! No server and no network: the backend's `rsh` hook points at a script that
//! prints one of OpenSSH's real messages and exits with one of its real codes.
//! That is the whole failure being tested — the message `ssh` writes and the
//! error the app makes of it — and a real refused login would test it no
//! better while needing a second machine to refuse from.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use excalibur_engine::backend::xsync::{SshTarget, XsyncBackend};
use excalibur_engine::error::Surface;
use excalibur_engine::{EngineError, HostKey};

/// A stand-in `ssh` that prints `stderr` and exits with `code`.
///
/// The text is passed through a file rather than baked into the script, so a
/// message with quotes in it is the message OpenSSH actually prints.
fn fake_ssh(stderr: &str, code: i32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("ssh");
    std::fs::write(format!("{}.stderr", script.display()), stderr).expect("stderr text");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ncat \"$0.stderr\" >&2\nexit {code}\n"),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    dir
}

/// Connect through the fake `ssh` in `dir` and return the failure.
async fn failure(dir: &Path, known_hosts: &Path) -> EngineError {
    let script = dir.join("ssh");
    let target = SshTarget {
        host: "orion.local",
        port: 22,
        username: Some("someone"),
        known_hosts,
        rsh: Some(script.to_str().expect("utf-8 path")),
    };
    XsyncBackend::connect_ssh(&target, "/srv/strife", None)
        .await
        .err()
        .expect("a fake ssh never answers the handshake")
}

/// A `known_hosts` path with nothing in it.
fn no_known_hosts(dir: &Path) -> std::path::PathBuf {
    dir.join("known_hosts")
}

#[tokio::test]
async fn a_rejected_login_is_an_authentication_failure_not_a_lost_connection() {
    // The bug this story exists for: `ssh` said the key was refused and the
    // user was told the connection dropped, which is a different problem with
    // a different fix — and, being transient, one the session manager would
    // have retried against the server until the account locked (E15-S2).
    let dir = fake_ssh("someone@orion.local: Permission denied (publickey).", 255);
    let error = failure(dir.path(), &no_known_hosts(dir.path())).await;

    assert!(
        matches!(error, EngineError::AuthenticationFailed(_)),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("Permission denied (publickey)"),
        "the message drops what ssh said: {error}"
    );
    assert!(!error.transient(), "a refused key must not be retried");
    assert_eq!(error.surface(), Surface::Banner);
}

#[tokio::test]
async fn a_password_prompt_refused_in_batch_mode_is_also_an_authentication_failure() {
    // No user@host in this one, so the message has to name the host itself or
    // it is a sentence about nothing.
    let dir = fake_ssh("Permission denied, please try again.", 255);
    let error = failure(dir.path(), &no_known_hosts(dir.path())).await;

    assert!(
        matches!(error, EngineError::AuthenticationFailed(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("orion.local"), "{error}");
}

#[tokio::test]
async fn a_host_key_ssh_refuses_names_both_keys() {
    // `ssh` and this app keep separate trust stores, so reaching this means
    // they disagree. The message is only useful if it says which key each of
    // them meant, so it carries the one ssh saw and the one on file here.
    let dir = fake_ssh(
        "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
         @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
         The fingerprint for the ED25519 key sent by the remote host is\n\
         SHA256:pRJUcCU0Qw1cQvGa5xLXcHkE2sCzRsyDkzsJc9C4mI0.\n\
         Host key verification failed.",
        255,
    );
    let recorded = HostKey {
        host: "orion.local".to_owned(),
        port: 22,
        algorithm: "ssh-ed25519".to_owned(),
        key: "AAAAC3NzaC1lZDI1NTE5AAAAIJ4uSjxJ8dJk1WhVGpZ4iLPvDZ0mQ0RS0hVvJmZ0dV6z".to_owned(),
    };
    let known_hosts = dir.path().join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!("orion.local {} {}\n", recorded.algorithm, recorded.key),
    )
    .expect("known_hosts");

    let error = failure(dir.path(), &known_hosts).await;

    assert!(error.host_key(), "{error:?}");
    assert!(
        !error.transient(),
        "a key problem is never fixed by waiting"
    );
    let message = error.to_string();
    assert!(
        message.contains("SHA256:pRJUcCU0Qw1cQvGa5xLXcHkE2sCzRsyDkzsJc9C4mI0"),
        "{message}"
    );
    assert!(
        message.contains(&recorded.fingerprint()),
        "the message does not say which key was expected: {message}"
    );
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_not_reported_as_a_missing_file() {
    let dir = fake_ssh(
        "ssh: Could not resolve hostname orion.local: nodename nor servname provided, or not known",
        255,
    );
    let error = failure(dir.path(), &no_known_hosts(dir.path())).await;

    assert!(matches!(error, EngineError::HostNotFound(_)), "{error:?}");
    assert!(
        error.to_string().contains("Could not resolve hostname"),
        "{error}"
    );
    // Retrying a name that does not resolve is the loop E15-S2 stops, and the
    // banner would hide it behind the word "reconnecting".
    assert!(!error.transient());
}

#[tokio::test]
async fn a_refused_connection_is_the_one_class_that_stays_retryable() {
    // The counterweight to every assertion above: a server that is merely down
    // must still reconnect on its own, or the guard would be worse than the
    // loop it replaced.
    let dir = fake_ssh(
        "ssh: connect to host orion.local port 22: Connection refused",
        255,
    );
    let error = failure(dir.path(), &no_known_hosts(dir.path())).await;

    assert!(matches!(error, EngineError::Disconnected(_)), "{error:?}");
    assert!(error.transient(), "a host that is down is worth retrying");
    assert!(error.to_string().contains("Connection refused"), "{error}");
}

#[tokio::test]
async fn a_missing_xs_says_so_and_names_the_fix() {
    // 127 and the wording both, because which of the two arrives depends on
    // the remote login shell.
    let dir = fake_ssh("bash: line 1: xs: command not found", 127);
    let error = failure(dir.path(), &no_known_hosts(dir.path())).await;

    assert!(matches!(error, EngineError::NotFound(_)), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("xs on orion.local"), "{message}");
    assert!(
        message.contains("install xs on the server or put it on PATH"),
        "the message does not say what to do: {message}"
    );
}

#[tokio::test]
async fn an_unrecognised_failure_keeps_what_ssh_said() {
    // The phrases above are the ones OpenSSH has today. Anything else stays a
    // dropped connection — but carrying the text, because the failure this
    // story fixes was one nobody could see at all.
    let dir = fake_ssh("ssh: a phrase this app has never heard of", 255);
    let error = failure(dir.path(), &no_known_hosts(dir.path())).await;

    assert!(matches!(error, EngineError::Disconnected(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("a phrase this app has never heard of"),
        "{error}"
    );
}

#[tokio::test]
async fn a_chatty_remote_cannot_grow_the_message_without_limit() {
    // The drain lives as long as the session, so the buffer behind it is the
    // one place a remote could make this app hold an unbounded amount of its
    // output.
    let mut noise = String::new();
    for line in 0..500 {
        use std::fmt::Write as _;
        writeln!(noise, "{line}: {}", "x".repeat(4096)).expect("a String never fails to write");
    }
    let dir = fake_ssh(&format!("{noise}ssh: the last word"), 255);
    let error = failure(dir.path(), &no_known_hosts(dir.path())).await;

    let message = error.to_string();
    assert!(
        message.len() < 2048,
        "unbounded message: {} bytes",
        message.len()
    );
    assert!(
        message.contains("the last word"),
        "the bound dropped the newest line rather than the oldest: {message}"
    );
}
