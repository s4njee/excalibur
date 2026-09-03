//! Credentials, and the rules for handling them.
//!
//! Three properties this module exists to enforce, all of them easier to keep
//! by construction than by discipline:
//!
//! - **A secret never goes back to the webview.** There is no read command in
//!   the shell, and no way to ask for one: the engine fetches a credential when
//!   it connects, and the window never sees it again after the user typed it.
//! - **A secret is not in a `Debug` string.** [`Secret`] redacts itself, so a
//!   password cannot reach a log by way of `{:?}` on some struct that contains
//!   one three levels down.
//! - **A secret does not outlive its use.** The buffer is zeroized on drop.
//!
//! None of that makes a process memory dump safe. It makes the ordinary
//! accidents — a log line, a panic message, a serialized error — not leak.

use std::fmt;

use zeroize::Zeroizing;

use crate::error::{EngineError, Result};

/// The keychain service name every credential is filed under.
const SERVICE: &str = "excalibur";

/// A probe account, so availability can be answered by trying rather than
/// guessing. Named to be obvious if a human ever sees it in Keychain Access.
const PROBE_ACCOUNT: &str = "excalibur-keychain-probe";

/// A password, passphrase or token held in memory.
///
/// Cloneable, because a caller often needs to hand one to a backend while
/// keeping its own; every copy zeroizes itself.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Take ownership of a secret value.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// The secret itself.
    ///
    /// Named `expose` rather than `as_str` so that every place a credential
    /// leaves this type is visible when reading the code, and greppable.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether there is anything here. Useful without exposing it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The whole point: a struct containing a Secret can be logged with
        // `{:?}` without the password going with it.
        formatter.write_str("Secret(<redacted>)")
    }
}

/// Where credentials are kept.
pub trait SecretStore: Send + Sync {
    /// Whether this store can actually hold anything.
    ///
    /// A Linux desktop with no Secret Service has a keyring API that accepts
    /// calls and stores nothing. The UI needs to know before it offers to save.
    fn available(&self) -> bool;

    /// Store `secret` for `account`, replacing anything already there.
    ///
    /// # Errors
    /// Returns [`EngineError::Io`] when the platform store refuses.
    fn set(&self, account: &str, secret: &Secret) -> Result<()>;

    /// Fetch the secret for `account`, or `None` when there is none.
    ///
    /// # Errors
    /// Returns [`EngineError::Io`] when the platform store refuses.
    fn get(&self, account: &str) -> Result<Option<Secret>>;

    /// Remove the secret for `account`. Removing one that is not there is not
    /// an error: the caller's intent is that it be gone.
    ///
    /// # Errors
    /// Returns [`EngineError::Io`] when the platform store refuses.
    fn delete(&self, account: &str) -> Result<()>;
}

/// The OS credential store: Keychain, Secret Service, or Credential Manager.
#[derive(Debug, Default)]
pub struct KeyringStore;

impl KeyringStore {
    fn entry(account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, account)
            .map_err(|error| EngineError::Io(format!("keychain unavailable: {error}")))
    }
}

impl SecretStore for KeyringStore {
    fn available(&self) -> bool {
        // Answered by trying, because the failure this guards against is a
        // store that accepts a write and loses it. A round trip is the only
        // honest test, and it runs once at startup.
        let Ok(entry) = Self::entry(PROBE_ACCOUNT) else {
            return false;
        };
        if entry.set_password("probe").is_err() {
            return false;
        }
        let stored = entry.get_password().is_ok_and(|value| value == "probe");
        let _ = entry.delete_credential();
        stored
    }

    fn set(&self, account: &str, secret: &Secret) -> Result<()> {
        Self::entry(account)?
            .set_password(secret.expose())
            .map_err(|error| EngineError::Io(format!("could not save to the keychain: {error}")))
    }

    fn get(&self, account: &str) -> Result<Option<Secret>> {
        match Self::entry(account)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(EngineError::Io(format!(
                "could not read from the keychain: {error}"
            ))),
        }
    }

    fn delete(&self, account: &str) -> Result<()> {
        match Self::entry(account)?.delete_credential() {
            // Already gone is the outcome the caller asked for.
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(EngineError::Io(format!(
                "could not remove from the keychain: {error}"
            ))),
        }
    }
}

/// An in-memory store, for tests and for a desktop with no credential service.
///
/// Deliberately not a silent fallback: a caller that gets one of these should
/// tell the user their credentials will not be remembered, which is what
/// [`SecretStore::available`] reporting `false` is for.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Report unavailable, to exercise the no-keychain path.
    unavailable: bool,
}

impl MemorySecretStore {
    /// A store that reports itself unavailable, as a Linux box without a
    /// Secret Service does.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            entries: std::sync::Mutex::default(),
            unavailable: true,
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn available(&self) -> bool {
        !self.unavailable
    }

    fn set(&self, account: &str, secret: &Secret) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| EngineError::Io("secret store is poisoned".to_owned()))?
            .insert(account.to_owned(), secret.expose().to_owned());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<Secret>> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| EngineError::Io("secret store is poisoned".to_owned()))?
            .get(account)
            .map(|value| Secret::new(value.clone())))
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| EngineError::Io("secret store is poisoned".to_owned()))?
            .remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MemorySecretStore, Secret, SecretStore};

    #[test]
    fn a_secret_does_not_appear_in_its_own_debug_output() {
        let secret = Secret::new("hunter2".to_owned());
        // The accident this prevents: some struct three levels up gets logged
        // with `{:?}` and takes the password with it.
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert!(!format!("{secret:?}").contains("hunter2"));
        // It is still usable where it is meant to be used.
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn a_secret_inside_another_type_is_redacted_too() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Credentials {
            username: String,
            password: Secret,
        }
        let rendered = format!(
            "{:?}",
            Credentials {
                username: "dorian".to_owned(),
                password: Secret::new("hunter2".to_owned()),
            }
        );
        assert!(rendered.contains("dorian"), "the username is not a secret");
        assert!(!rendered.contains("hunter2"));
    }

    #[test]
    fn a_store_round_trips_and_forgets_on_request() {
        let store = MemorySecretStore::default();
        assert!(store.available());
        assert!(store.get("nas").unwrap().is_none());

        store
            .set("nas", &Secret::new("hunter2".to_owned()))
            .unwrap();
        assert_eq!(store.get("nas").unwrap().unwrap().expose(), "hunter2");

        // Replacing, not appending.
        store.set("nas", &Secret::new("other".to_owned())).unwrap();
        assert_eq!(store.get("nas").unwrap().unwrap().expose(), "other");

        store.delete("nas").unwrap();
        assert!(store.get("nas").unwrap().is_none());
        // Deleting what is not there is the outcome the caller asked for.
        assert!(store.delete("nas").is_ok());
    }

    #[test]
    fn an_unavailable_store_says_so_rather_than_failing_quietly() {
        // A Linux desktop with no Secret Service. The UI needs to disable the
        // checkbox rather than offer to save into nothing.
        let store = MemorySecretStore::unavailable();
        assert!(!store.available());
    }
}
