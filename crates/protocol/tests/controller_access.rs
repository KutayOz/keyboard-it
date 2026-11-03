use protocol::{
    access::{AccessStore, LEGACY_ID, MAX_CONTROLLERS},
    secure,
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
};

fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    (client, server)
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("keyboard-it-access-{}", secure::generate_key()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn policy(&self) -> PathBuf {
        self.0.join("access.toml")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn individual_revoke_closes_only_that_controller_and_rejects_stale_handshakes() {
    let access = AccessStore::in_memory("old-shared-key");
    let a = access.grant("Same display name", "device-a").unwrap();
    let b = access.grant("Same display name", "device-b").unwrap();
    assert_ne!(a, b);
    let keys = access.authentication_keys();
    let key_a = keys
        .iter()
        .find(|k| k.key == secure::psk_from_secret(&a))
        .unwrap();
    let key_b = keys
        .iter()
        .find(|k| k.key == secure::psk_from_secret(&b))
        .unwrap();
    let (mut client_a, server_a) = sockets();
    let (mut client_b, mut server_b) = sockets();
    let lease_a = access.attach(key_a, &server_a).unwrap();
    let _lease_b = access.attach(key_b, &server_b).unwrap();
    access.revoke(&key_a.id).unwrap();
    let mut dispatched = false;
    assert!(lease_a.dispatch(|| dispatched = true).is_err());
    assert!(!dispatched);
    assert_eq!(client_a.read(&mut [0]).unwrap(), 0);
    client_b.write_all(b"b").unwrap();
    let mut buf = [0];
    server_b.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"b");
    let mut published = false;
    assert!(access
        .attach_with(key_a, &server_a, || {
            published = true;
            Ok(())
        })
        .is_err());
    assert!(!published);
    assert!(access
        .authentication_keys()
        .iter()
        .any(|k| k.id == key_b.id));
    assert!(access
        .authentication_keys()
        .iter()
        .any(|k| k.id == LEGACY_ID));
}

#[test]
fn repair_rotates_only_the_approved_identity_and_anonymous_names_do_not_deduplicate() {
    let access = AccessStore::in_memory("");
    let original = access.grant("Laptop", "stable-a").unwrap();
    let old = access.authentication_keys()[0].clone();
    let repaired = access.grant("Renamed laptop", "stable-a").unwrap();
    assert_ne!(original, repaired);
    assert_eq!(access.controllers().len(), 1);
    assert_eq!(access.controllers()[0].id, old.id);
    assert_eq!(access.controllers()[0].name, "Renamed laptop");
    let (_, socket) = sockets();
    assert!(access.attach(&old, &socket).is_err());
    access.grant("Laptop", "").unwrap();
    access.grant("Laptop", "").unwrap();
    assert_eq!(access.controllers().len(), 3);
}

#[test]
fn revoked_legacy_access_stays_revoked_after_restart_and_settings_saves() {
    let fixture = Fixture::new();
    let store = AccessStore::open_path(&fixture.policy(), "legacy-secret").unwrap();
    let granted = store.grant("New controller", "a").unwrap();
    store.revoke(LEGACY_ID).unwrap();
    drop(store);
    let restored = AccessStore::open_path(&fixture.policy(), "legacy-secret").unwrap();
    assert!(restored.controllers().iter().all(|c| !c.legacy));
    assert_eq!(
        restored.authentication_keys()[0].key,
        secure::psk_from_secret(&granted)
    );
    restored.revoke_all().unwrap();
    drop(restored);
    let restored = AccessStore::open_path(&fixture.policy(), "same-or-new-config-secret").unwrap();
    assert!(restored.authentication_keys().is_empty());
    restored.grant("Allowed again", "a").unwrap();
    assert_eq!(restored.controllers().len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(fixture.policy())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn a_second_policy_owner_cannot_overwrite_the_running_hosts_cached_access() {
    let fixture = Fixture::new();
    let store = AccessStore::open_path(&fixture.policy(), "").unwrap();
    let key = store.grant("Controller", "a").unwrap();
    let error = AccessStore::open_path(&fixture.policy(), "").err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert_eq!(
        store.authentication_keys()[0].key,
        secure::psk_from_secret(&key)
    );
    drop(store);
    let reopened = AccessStore::open_path(&fixture.policy(), "").unwrap();
    assert_eq!(reopened.controllers().len(), 1);
}

#[test]
fn failed_policy_writes_preserve_live_access_and_do_not_grant_unpersisted_keys() {
    let fixture = Fixture::new();
    let path = fixture.0.join("child").join("access.toml");
    let store = AccessStore::open_path(&path, "").unwrap();
    store.grant("A", "a").unwrap();
    let key = store.authentication_keys()[0].clone();
    let (mut client, mut server) = sockets();
    let _lease = store.attach(&key, &server).unwrap();
    // Obstruct the atomic replacement without moving the live lock file's
    // parent: Windows disallows that rename while the policy owner is open.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(store.revoke(&key.id).is_err());
    assert!(store.grant("B", "b").is_err());
    assert!(store.revoke_all().is_err());
    assert_eq!(store.controllers().len(), 1);
    client.write_all(b"x").unwrap();
    server.read_exact(&mut [0]).unwrap();
}

#[test]
fn invalid_policy_fails_closed_and_keyring_is_bounded() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.policy(),
        "version=999\nlegacy_allowed=true\ncontrollers=[]",
    )
    .unwrap();
    assert!(AccessStore::open_path(&fixture.policy(), "legacy").is_err());
    let store = AccessStore::in_memory("");
    for i in 0..MAX_CONTROLLERS {
        store.grant("PC", &format!("id-{i}")).unwrap();
    }
    assert!(store.grant("extra", "extra").is_err());
    assert!(store.grant("repair", "id-0").is_ok());
    assert!(store.grant("bad identity", "bad\nidentity").is_err());
}

#[test]
fn revocation_waits_for_inflight_dispatch_then_blocks_subsequent_actions() {
    use std::{sync::mpsc, thread, time::Duration};
    let store = AccessStore::in_memory("");
    store.grant("A", "a").unwrap();
    let key = store.authentication_keys()[0].clone();
    let (_, socket) = sockets();
    let lease = store.attach(&key, &socket).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let active = lease.clone();
    let action = thread::spawn(move || {
        active
            .dispatch(|| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap()
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let revoker = thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        store.revoke(&key.id).unwrap();
        done_tx.send(()).unwrap();
    });
    attempt_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(done_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    action.join().unwrap();
    revoker.join().unwrap();
    assert!(lease
        .dispatch(|| panic!("revoked action executed"))
        .is_err());
}

#[test]
fn an_old_approval_cannot_restore_access_after_revoke_all() {
    let store = AccessStore::in_memory("");
    let approval = store.revision();
    store.revoke_all().unwrap();
    assert!(store
        .grant_if_revision("Late approval", "a", approval)
        .is_err());
    assert!(store.controllers().is_empty());
    store
        .grant_if_revision("Fresh approval", "a", store.revision())
        .unwrap();
    assert_eq!(store.controllers().len(), 1);
}
