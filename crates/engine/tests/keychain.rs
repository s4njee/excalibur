//! The real OS credential store.
//!
//! Opt-in with `EXCALIBUR_KEYCHAIN_TEST=1`: this writes to the login keychain
//! of whoever runs it, and a test suite should not touch a developer's
//! credential store without being asked.

use excalibur_engine::{KeyringStore, Secret, SecretStore};

#[test]
fn the_platform_store_round_trips_a_credential() {
    if std::env::var("EXCALIBUR_KEYCHAIN_TEST").is_err() {
        eprintln!("skipping: set EXCALIBUR_KEYCHAIN_TEST=1 to touch the real keychain");
        return;
    }
    let store = KeyringStore;
    if !store.available() {
        eprintln!("skipping: no credential service on this machine");
        return;
    }

    let account = "excalibur-test-account";
    // Clean first, in case a previous run was interrupted.
    store.delete(account).expect("clean");
    assert!(store.get(account).expect("read").is_none());

    store
        .set(account, &Secret::new("hunter2".to_owned()))
        .expect("write");
    assert_eq!(
        store
            .get(account)
            .expect("read back")
            .expect("present")
            .expose(),
        "hunter2"
    );

    store.delete(account).expect("remove");
    assert!(
        store.get(account).expect("read after remove").is_none(),
        "a deleted credential must be gone, not merely unreadable"
    );
}
