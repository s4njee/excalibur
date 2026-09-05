//! Host key trust for the protocols that run over SSH.
//!
//! The problem this solves is concrete. `xs` is reached by spawning `ssh`, and
//! `ssh` verifies the host key itself — by asking on the terminal. A GUI app
//! has no terminal, so OpenSSH's prompt cannot be answered and the connection
//! dies with `Host key verification failed.` on a stderr nobody is reading.
//! Every first connection to a new host failed that way, invisibly.
//!
//! So Excalibur takes the decision away from `ssh` and makes it visible:
//!
//! 1. [`scan`] asks the host for its keys with `ssh-keyscan`, without
//!    authenticating and without trusting anything.
//! 2. [`verify`] compares what came back against [`KnownHosts`] — a
//!    `known_hosts`-compatible file in the app's data directory — and answers
//!    [`Trust::Trusted`], [`Trust::Unknown`] or [`Trust::Changed`].
//! 3. The window shows a prompt for `Unknown`, and a red banner with no way
//!    through for `Changed`.
//! 4. The connection itself then runs with `StrictHostKeyChecking=yes` against
//!    that same file, so `ssh` enforces the decision the user actually made.
//!
//! Step 4 is what makes step 1 safe. `ssh-keyscan` opens its own connection, so
//! an attacker could in principle answer the scan with the real key and the
//! session with their own. They gain nothing: the session is checked against
//! the key we stored from the scan, and a mismatch fails the connection. The
//! scan decides *what to ask the user about*; it never decides what to trust.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use sha2::{Digest as _, Sha256};

use crate::error::{EngineError, Result};

/// The default SSH port, and the one `known_hosts` leaves implicit.
pub const DEFAULT_SSH_PORT: u16 = 22;

/// One host key as `known_hosts` records it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKey {
    /// Hostname as the user typed it. Matching is case-insensitive.
    pub host: String,
    /// Port the key belongs to.
    pub port: u16,
    /// Key type, e.g. `ssh-ed25519`.
    pub algorithm: String,
    /// The key itself, base64 as `known_hosts` stores it.
    pub key: String,
}

impl HostKey {
    /// The fingerprint OpenSSH would print: `SHA256:` and the digest of the raw
    /// key, base64 without padding.
    ///
    /// This is the string the user is asked to compare against what the server
    /// operator told them, so it has to match `ssh-keygen -lf` exactly —
    /// character for character, or the comparison is worthless. There is a test
    /// pinning it to a key whose fingerprint was taken from `ssh-keygen`.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(&self.key) else {
            // A key we cannot decode is one we can never match, so it can never
            // be trusted. Say so in the place the user would read.
            return "SHA256:<unreadable key>".to_owned();
        };
        let digest = Sha256::digest(&raw);
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
        format!("SHA256:{encoded}")
    }

    /// How `known_hosts` names this host: bare, or `[host]:port` off port 22.
    fn pattern(&self) -> String {
        if self.port == DEFAULT_SSH_PORT {
            self.host.to_lowercase()
        } else {
            format!("[{}]:{}", self.host.to_lowercase(), self.port)
        }
    }

    /// One `known_hosts` line, without the newline.
    fn to_line(&self) -> String {
        format!("{} {} {}", self.pattern(), self.algorithm, self.key)
    }
}

/// What [`verify`] concluded about a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// Every key the host offered is one we have already accepted.
    Trusted,
    /// We have nothing on file for this host. Ask the user.
    Unknown {
        /// The key to show in the prompt, and to store if they accept.
        offered: HostKey,
    },
    /// We have a key on file and the host offered a different one.
    ///
    /// This is the case that must not be click-through. It is what a
    /// man-in-the-middle looks like, and it is also what a rebuilt server looks
    /// like — the user cannot tell those apart from a dialog, so the answer is
    /// to stop and make them go and find out.
    Changed {
        /// What the host offered now.
        offered: HostKey,
        /// What we accepted previously, for the same algorithm.
        stored: HostKey,
    },
    /// The key is on file as revoked.
    Revoked {
        /// The revoked key the host offered.
        offered: HostKey,
    },
}

/// A `known_hosts`-compatible file.
///
/// Compatible, but deliberately not *shared*: this never reads or writes the
/// user's `~/.ssh/known_hosts`. Two reasons, and the second is the real one.
/// Writing to it would mean an app silently editing the file every other SSH
/// tool on the machine depends on. And reading it correctly means implementing
/// hashed entries (`HashKnownHosts` is on by default on most Linux distros),
/// which is HMAC-SHA1 over a salt — getting that subtly wrong would either
/// hard-fail a host whose key never changed, or fail to notice one that did.
/// Asking the user once more is a far cheaper mistake than either of those.
#[derive(Debug, Clone)]
pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    /// A store backed by a specific file. The file need not exist yet.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The store in the app's data directory.
    ///
    /// # Errors
    ///
    /// Fails when the platform has no data directory to put it in.
    pub fn app_default() -> Result<Self> {
        let base = dirs::data_dir()
            .ok_or_else(|| EngineError::Io("this platform has no data directory".to_owned()))?;
        Ok(Self::new(base.join("excalibur").join("known_hosts")))
    }

    /// Where the file lives, for the ssh command line and for error messages.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every key on file for a host and port.
    ///
    /// A missing file is an empty store, not an error: nothing has been trusted
    /// yet, which is the normal state on first run.
    ///
    /// # Errors
    ///
    /// Fails when the file exists but cannot be read.
    pub fn stored(&self, host: &str, port: u16) -> Result<Vec<StoredKey>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(EngineError::Io(format!(
                    "could not read {}: {error}",
                    self.path.display()
                )))
            }
        };
        let wanted = HostKey {
            host: host.to_owned(),
            port,
            algorithm: String::new(),
            key: String::new(),
        }
        .pattern();
        Ok(text
            .lines()
            .filter_map(parse_line)
            .filter(|entry| entry.matches(&wanted))
            .collect())
    }

    /// Record a key as trusted.
    ///
    /// Appending rather than replacing is what `known_hosts` does, and it is
    /// right here too: a host legitimately offers several keys of different
    /// algorithms, and trusting its Ed25519 key says nothing about its RSA one.
    ///
    /// # Errors
    ///
    /// Fails when the file or its directory cannot be written.
    pub fn trust(&self, key: &HostKey) -> Result<()> {
        if self
            .stored(&key.host, key.port)?
            .iter()
            .any(|entry| entry.key.algorithm == key.algorithm && entry.key.key == key.key)
        {
            return Ok(());
        }
        let mut text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(EngineError::Io(format!(
                    "could not read {}: {error}",
                    self.path.display()
                )))
            }
        };
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&key.to_line());
        text.push('\n');
        self.write(&text)
    }

    /// Drop every key for a host, so the next connection asks again.
    ///
    /// This is the "deliberate second step" a changed key needs: it is reached
    /// from the bookmark editor, never from the banner that reports the change.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or written.
    pub fn forget(&self, host: &str, port: u16) -> Result<()> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(EngineError::Io(format!(
                    "could not read {}: {error}",
                    self.path.display()
                )))
            }
        };
        let wanted = HostKey {
            host: host.to_owned(),
            port,
            algorithm: String::new(),
            key: String::new(),
        }
        .pattern();
        let kept: Vec<&str> = text
            .lines()
            .filter(|line| parse_line(line).is_none_or(|entry| !entry.matches(&wanted)))
            .collect();
        let mut out = kept.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        self.write(&out)
    }

    /// Writes through a temporary file in the same directory, then renames.
    ///
    /// A half-written `known_hosts` is worse than none: it would silently drop
    /// the trust decisions past the truncation point, and the next connection
    /// would report a *changed* key rather than an unknown one — sending the
    /// user to look for an attacker that was really a crash.
    fn write(&self, text: &str) -> Result<()> {
        let parent = self.path.parent().ok_or_else(|| {
            EngineError::Io(format!("{} has no parent directory", self.path.display()))
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            EngineError::Io(format!("could not create {}: {error}", parent.display()))
        })?;
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, text).map_err(|error| {
            EngineError::Io(format!("could not write {}: {error}", temporary.display()))
        })?;
        std::fs::rename(&temporary, &self.path).map_err(|error| {
            EngineError::Io(format!(
                "could not replace {}: {error}",
                self.path.display()
            ))
        })
    }
}

/// A parsed `known_hosts` entry: the key, plus whether it is a plain one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredKey {
    /// The key itself.
    pub key: HostKey,
    /// Set by an `@revoked` marker. Such a key is never trusted.
    pub revoked: bool,
    /// The host patterns this line applies to, lower-cased.
    patterns: Vec<String>,
}

impl StoredKey {
    fn matches(&self, pattern: &str) -> bool {
        self.patterns.iter().any(|candidate| candidate == pattern)
    }
}

/// Parses one `known_hosts` line, or `None` if it carries no usable key.
///
/// Hashed entries (`|1|salt|hash`) and `@cert-authority` lines are skipped
/// rather than guessed at — see [`KnownHosts`] for why. Skipping a line makes
/// the host look unknown, which prompts; misreading one could make a changed
/// key look trusted, which is the failure that matters.
fn parse_line(line: &str) -> Option<StoredKey> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields = line.split_whitespace();
    let mut first = fields.next()?;
    let mut revoked = false;
    if first.starts_with('@') {
        match first {
            "@revoked" => revoked = true,
            // Certificate authorities need certificate validation to mean
            // anything, which this does not do.
            _ => return None,
        }
        first = fields.next()?;
    }
    if first.starts_with('|') {
        return None;
    }
    let algorithm = fields.next()?;
    let key = fields.next()?;
    let patterns: Vec<String> = first.split(',').map(str::to_lowercase).collect();
    Some(StoredKey {
        key: HostKey {
            host: patterns.first()?.clone(),
            port: DEFAULT_SSH_PORT,
            algorithm: algorithm.to_owned(),
            key: key.to_owned(),
        },
        revoked,
        patterns,
    })
}

/// Key algorithms, best first.
///
/// This is the order the prompt picks from when a host offers several, and it
/// tracks OpenSSH's own default preference. It matters because the key we show
/// the user is the key we store, and the key we store is the one `ssh` will be
/// held to.
const PREFERENCE: [&str; 4] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "rsa-sha2-512",
    "ssh-rsa",
];

/// Picks the key to show the user out of everything a host offered.
fn preferred(offered: &[HostKey]) -> Option<&HostKey> {
    PREFERENCE
        .iter()
        .find_map(|algorithm| offered.iter().find(|key| key.algorithm == *algorithm))
        .or_else(|| offered.first())
}

/// Asks a host for its public keys, without authenticating to it.
///
/// This is a trait so the tests can decide what a host offers. Talking to a
/// real `ssh-keyscan` in a unit test would make the suite depend on a network
/// and on a server whose keys can change underneath it.
#[async_trait::async_trait]
pub trait HostKeyScanner: Send + Sync {
    /// Every key `host:port` offers.
    ///
    /// # Errors
    ///
    /// Fails when the host cannot be reached or the scanner cannot be run.
    async fn scan(&self, host: &str, port: u16) -> Result<Vec<HostKey>>;
}

/// Scans with OpenSSH's own `ssh-keyscan`.
///
/// Shelling out rather than speaking SSH ourselves is the right trade here: the
/// binary ships everywhere `ssh` does, it is the tool written for exactly this,
/// and the alternative is an SSH implementation whose only job is to hang up
/// before authenticating.
#[derive(Debug, Clone, Copy, Default)]
pub struct Keyscan;

#[async_trait::async_trait]
impl HostKeyScanner for Keyscan {
    async fn scan(&self, host: &str, port: u16) -> Result<Vec<HostKey>> {
        let output = tokio::process::Command::new("ssh-keyscan")
            .arg("-T")
            .arg("5")
            .arg("-p")
            .arg(port.to_string())
            .arg(host)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|error| EngineError::Io(format!("could not run ssh-keyscan: {error}")))?;
        let keys = parse_scan(&String::from_utf8_lossy(&output.stdout), host, port);
        if keys.is_empty() {
            // ssh-keyscan exits 0 with no output for a host that never
            // answered, so the exit status is not the signal here. Its stderr
            // is the only account of what happened, and the user needs it: it
            // distinguishes a typo'd hostname from a firewall.
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            // A name that does not resolve is not a host that is down, and
            // must not be retried as one: `Disconnected` is transient and
            // E15-S2's reconnect loop would chase a typo forever. The
            // phrases are getaddrinfo's on macOS and glibc respectively.
            if detail.contains("nodename nor servname")
                || detail.contains("Name or service not known")
                || detail.contains("Temporary failure in name resolution")
                || detail.contains("Could not resolve hostname")
            {
                return Err(EngineError::HostNotFound(format!(
                    "{host} could not be resolved ({detail})"
                )));
            }
            return Err(EngineError::Disconnected(if detail.is_empty() {
                format!("{host}:{port} offered no SSH host keys")
            } else {
                detail.to_owned()
            }));
        }
        Ok(keys)
    }
}

/// Parses `ssh-keyscan` output into keys for a known host and port.
///
/// The host and port come from the caller rather than from the output: what
/// `ssh-keyscan` prints in that field is whatever it resolved, and the trust
/// decision has to be recorded against the name the user actually typed and
/// will type again.
fn parse_scan(stdout: &str, host: &str, port: u16) -> Vec<HostKey> {
    let mut seen = HashSet::new();
    stdout
        .lines()
        .filter_map(parse_line)
        .filter(|entry| !entry.revoked)
        .filter(|entry| seen.insert((entry.key.algorithm.clone(), entry.key.key.clone())))
        .map(|entry| HostKey {
            host: host.to_owned(),
            port,
            algorithm: entry.key.algorithm,
            key: entry.key.key,
        })
        .collect()
}

/// Decides whether a host may be connected to.
///
/// # Errors
///
/// Fails when the host cannot be scanned, or when the store cannot be read.
pub async fn verify(
    scanner: &dyn HostKeyScanner,
    known: &KnownHosts,
    host: &str,
    port: u16,
) -> Result<Trust> {
    let offered = scanner.scan(host, port).await?;
    check(known, host, port, &offered)
}

/// Decide about keys the caller already has.
///
/// Split out from [`verify`] because an SSH library hands the host key over
/// during its own handshake — there is nothing to scan for. Both paths reach
/// the same decision through this one function, which matters more than it
/// looks: two implementations of "is this key trusted" is exactly the sort of
/// duplication where one of them ends up more permissive than the other.
///
/// # Errors
///
/// Fails when the store cannot be read.
pub fn check(known: &KnownHosts, host: &str, port: u16, offered: &[HostKey]) -> Result<Trust> {
    let stored = known.stored(host, port)?;

    // Revocation is checked against every key the host offered, not just the
    // preferred one: a host that still answers with a revoked key has not
    // stopped using it, whatever else it also offers.
    if let Some(revoked) = stored.iter().find(|entry| {
        entry.revoked
            && offered
                .iter()
                .any(|key| key.algorithm == entry.key.algorithm && key.key == entry.key.key)
    }) {
        let algorithm = revoked.key.algorithm.clone();
        let key = revoked.key.key.clone();
        return Ok(Trust::Revoked {
            offered: HostKey {
                host: host.to_owned(),
                port,
                algorithm,
                key,
            },
        });
    }

    let live: Vec<&StoredKey> = stored.iter().filter(|entry| !entry.revoked).collect();
    if live.is_empty() {
        let Some(offered) = preferred(offered) else {
            return Err(EngineError::Disconnected(format!(
                "{host}:{port} offered no SSH host keys"
            )));
        };
        return Ok(Trust::Unknown {
            offered: offered.clone(),
        });
    }

    // A single match is enough, exactly as it is for ssh: the host proved it
    // holds a key we already accepted.
    if live
        .iter()
        .any(|entry| offered.iter().any(|key| key.key == entry.key.key))
    {
        return Ok(Trust::Trusted);
    }

    // Same algorithm, different key. That is the alarming case, and it is
    // reported per algorithm so the message can name which key changed.
    for entry in &live {
        if let Some(now) = offered
            .iter()
            .find(|key| key.algorithm == entry.key.algorithm)
        {
            return Ok(Trust::Changed {
                offered: now.clone(),
                stored: HostKey {
                    host: host.to_owned(),
                    port,
                    algorithm: entry.key.algorithm.clone(),
                    key: entry.key.key.clone(),
                },
            });
        }
    }

    // We hold keys for this host, but none of the algorithms it now offers.
    // A host that added a key type is far more likely than one that dropped
    // every type we knew, so this asks rather than alarms.
    let Some(offered) = preferred(offered) else {
        return Err(EngineError::Disconnected(format!(
            "{host}:{port} offered no SSH host keys"
        )));
    };
    Ok(Trust::Unknown {
        offered: offered.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Ed25519 host key, with the fingerprint `ssh-keygen -lf` prints
    /// for it. Pinning a known-good pair is the only way this test can catch a
    /// change in how the digest is derived or encoded.
    const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
    const ED25519_FINGERPRINT: &str = "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU";

    fn key(algorithm: &str, material: &str) -> HostKey {
        HostKey {
            host: "nas.local".to_owned(),
            port: 22,
            algorithm: algorithm.to_owned(),
            key: material.to_owned(),
        }
    }

    struct Offers(Vec<HostKey>);

    #[async_trait::async_trait]
    impl HostKeyScanner for Offers {
        async fn scan(&self, _host: &str, _port: u16) -> Result<Vec<HostKey>> {
            Ok(self.0.clone())
        }
    }

    fn store() -> (tempfile::TempDir, KnownHosts) {
        let dir = tempfile::tempdir().expect("temp dir");
        let known = KnownHosts::new(dir.path().join("known_hosts"));
        (dir, known)
    }

    #[test]
    fn the_fingerprint_matches_what_ssh_keygen_prints() {
        assert_eq!(
            key("ssh-ed25519", ED25519).fingerprint(),
            ED25519_FINGERPRINT
        );
    }

    #[test]
    fn an_undecodable_key_reports_rather_than_panics() {
        assert_eq!(
            key("ssh-ed25519", "not base64 at all!").fingerprint(),
            "SHA256:<unreadable key>"
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_store_not_an_error() {
        let (_dir, known) = store();
        assert!(known.stored("nas.local", 22).expect("read").is_empty());
    }

    #[test]
    fn a_trusted_key_round_trips_through_the_file() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        let stored = known.stored("nas.local", 22).expect("read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key.key, ED25519);
        assert!(!stored[0].revoked);
    }

    #[test]
    fn trusting_the_same_key_twice_does_not_duplicate_it() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        known.trust(&key("ssh-ed25519", ED25519)).expect("again");
        assert_eq!(known.stored("nas.local", 22).expect("read").len(), 1);
    }

    #[test]
    fn the_file_is_known_hosts_format() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        let text = std::fs::read_to_string(known.path()).expect("read");
        assert_eq!(text, format!("nas.local ssh-ed25519 {ED25519}\n"));
    }

    #[test]
    fn a_non_default_port_is_bracketed_and_does_not_match_the_default() {
        let (_dir, known) = store();
        let mut moved = key("ssh-ed25519", ED25519);
        moved.port = 2222;
        known.trust(&moved).expect("trust");

        let text = std::fs::read_to_string(known.path()).expect("read");
        assert!(text.starts_with("[nas.local]:2222 "), "got {text}");
        // The same host on the standard port is a different trust decision.
        assert!(known.stored("nas.local", 22).expect("read").is_empty());
        assert_eq!(known.stored("nas.local", 2222).expect("read").len(), 1);
    }

    #[test]
    fn a_host_is_matched_case_insensitively() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        assert_eq!(known.stored("NAS.Local", 22).expect("read").len(), 1);
    }

    #[test]
    fn forget_removes_one_host_and_leaves_the_others() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        let mut other = key("ssh-ed25519", ED25519);
        other.host = "other.local".to_owned();
        known.trust(&other).expect("trust");

        known.forget("nas.local", 22).expect("forget");

        assert!(known.stored("nas.local", 22).expect("read").is_empty());
        assert_eq!(known.stored("other.local", 22).expect("read").len(), 1);
    }

    #[test]
    fn comments_hashed_entries_and_authorities_are_skipped() {
        let (_dir, known) = store();
        std::fs::write(
            known.path(),
            format!(
                "# a comment\n\
                 |1|c2FsdA==|aGFzaA== ssh-ed25519 {ED25519}\n\
                 @cert-authority nas.local ssh-ed25519 {ED25519}\n"
            ),
        )
        .expect("write");
        assert!(known.stored("nas.local", 22).expect("read").is_empty());
    }

    #[tokio::test]
    async fn an_unseen_host_is_unknown_and_offers_its_best_key() {
        let (_dir, known) = store();
        let offers = Offers(vec![
            key("ssh-rsa", "cnNhIGtleQ=="),
            key("ssh-ed25519", ED25519),
        ]);

        let trust = verify(&offers, &known, "nas.local", 22)
            .await
            .expect("verify");

        // Ed25519 is preferred over RSA even though RSA came first.
        assert_eq!(
            trust,
            Trust::Unknown {
                offered: key("ssh-ed25519", ED25519)
            }
        );
    }

    #[tokio::test]
    async fn a_stored_key_the_host_still_offers_is_trusted() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        let offers = Offers(vec![key("ssh-ed25519", ED25519)]);

        assert_eq!(
            verify(&offers, &known, "nas.local", 22)
                .await
                .expect("verify"),
            Trust::Trusted
        );
    }

    #[tokio::test]
    async fn a_different_key_for_a_known_algorithm_is_a_change() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        let imposter = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAA",
        );
        let offers = Offers(vec![imposter.clone()]);

        let trust = verify(&offers, &known, "nas.local", 22)
            .await
            .expect("verify");

        assert_eq!(
            trust,
            Trust::Changed {
                offered: imposter,
                stored: key("ssh-ed25519", ED25519),
            }
        );
    }

    #[tokio::test]
    async fn a_host_that_added_an_algorithm_is_still_trusted() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        // The host now also offers RSA. The key we trusted is still there, so
        // this is not a change and must not alarm.
        let offers = Offers(vec![
            key("ssh-ed25519", ED25519),
            key("ssh-rsa", "cnNhIGtleQ=="),
        ]);

        assert_eq!(
            verify(&offers, &known, "nas.local", 22)
                .await
                .expect("verify"),
            Trust::Trusted
        );
    }

    #[tokio::test]
    async fn a_host_offering_only_unseen_algorithms_asks_rather_than_alarms() {
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");
        let offers = Offers(vec![key("ssh-rsa", "cnNhIGtleQ==")]);

        let trust = verify(&offers, &known, "nas.local", 22)
            .await
            .expect("verify");

        assert!(matches!(trust, Trust::Unknown { .. }), "got {trust:?}");
    }

    #[tokio::test]
    async fn a_revoked_key_is_refused_even_when_the_host_offers_another() {
        let (_dir, known) = store();
        std::fs::write(
            known.path(),
            format!("@revoked nas.local ssh-ed25519 {ED25519}\n"),
        )
        .expect("write");
        let offers = Offers(vec![
            key("ssh-ed25519", ED25519),
            key("ssh-rsa", "cnNhIGtleQ=="),
        ]);

        let trust = verify(&offers, &known, "nas.local", 22)
            .await
            .expect("verify");

        assert!(matches!(trust, Trust::Revoked { .. }), "got {trust:?}");
    }

    #[test]
    fn checking_a_key_in_hand_reaches_the_same_verdict_as_scanning_for_it() {
        // Two implementations of "is this key trusted" is exactly where one
        // ends up more permissive than the other, so `verify` and `check` are
        // the same decision reached two ways. An SSH library hands the key
        // over during its own handshake; there is nothing to scan for.
        let (_dir, known) = store();
        known.trust(&key("ssh-ed25519", ED25519)).expect("trust");

        let good = [key("ssh-ed25519", ED25519)];
        assert_eq!(
            check(&known, "nas.local", 22, &good).expect("check"),
            Trust::Trusted
        );

        let imposter = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAA",
        );
        assert!(matches!(
            check(&known, "nas.local", 22, &[imposter]).expect("check"),
            Trust::Changed { .. }
        ));

        assert!(matches!(
            check(&known, "other.local", 22, &good).expect("check"),
            Trust::Unknown { .. }
        ));
    }

    #[tokio::test]
    async fn a_name_that_does_not_resolve_is_not_a_host_that_is_down() {
        // `.invalid` is reserved never to resolve (RFC 2606), so this needs
        // no network and cannot start passing because DNS changed. The
        // point is the class: `HostNotFound` is not transient, so a typo in a
        // bookmark is shown once rather than reconnected forever.
        let error = Keyscan
            .scan("no-such-host.invalid", 22)
            .await
            .expect_err("a name that cannot resolve is an error");
        assert!(
            matches!(error, EngineError::HostNotFound(_)),
            "expected HostNotFound, got {error:?}"
        );
        assert!(!error.transient(), "{error:?} must not be retried");
    }

    #[test]
    fn scan_output_is_parsed_against_the_name_the_user_typed() {
        // ssh-keyscan reports what it resolved; the trust decision belongs to
        // the name that will be typed again.
        let keys = parse_scan(
            &format!("# nas.local:22 SSH-2.0-OpenSSH_9.6\n192.168.1.10 ssh-ed25519 {ED25519}\n"),
            "nas.local",
            2222,
        );
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].host, "nas.local");
        assert_eq!(keys[0].port, 2222);
    }

    #[test]
    fn duplicate_scan_lines_collapse() {
        let line = format!("nas.local ssh-ed25519 {ED25519}\n");
        let keys = parse_scan(&format!("{line}{line}"), "nas.local", 22);
        assert_eq!(keys.len(), 1);
    }
}
