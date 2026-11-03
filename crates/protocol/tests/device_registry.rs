use protocol::{
    config::Config,
    devices::{PairedDevice, Platform},
};

fn device(id: &str) -> PairedDevice {
    PairedDevice {
        transport: Default::default(),
        capabilities: None,
        id: id.into(),
        name: format!("PC {id}"),
        platform: Platform::Windows,
        permissions: Default::default(),
        peer_device_id: format!("identity-{id}"),
        shared_secret: format!("secret-{id}"),
        host: format!("{id}.local"),
        ip: String::new(),
        port: 5599,
        desktop_port: 5601,
    }
}

#[test]
fn legacy_migration_is_stable_and_keeps_key_and_address() {
    let text =
        "shared_secret='old-key'\npeer_host='pc.local'\npeer_ip='192.168.1.2'\nrole='sender'\n";
    let a = Config::from_toml(text).unwrap();
    let b = Config::from_toml(text).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.devices.len(), 1);
    assert_eq!(a.active_device().unwrap().shared_secret, "old-key");
    assert_eq!(a.active_device().unwrap().ip, "192.168.1.2");
    assert_eq!(Config::from_toml(&toml::to_string(&a).unwrap()).unwrap(), a);
    assert_eq!(
        Config::from_toml("role='receiver'\nshared_secret='host-key'")
            .unwrap()
            .devices
            .len(),
        0
    );
}

#[test]
fn adding_devices_preserves_active_target_and_switching_preserves_pairings() {
    let mut cfg = Config::default();
    cfg.remember_device(device("a")).unwrap();
    let original = cfg.session_target();
    cfg.remember_device(device("b")).unwrap();
    assert_eq!(cfg.session_target(), original);
    cfg.peer_ip = "192.168.1.77".into(); // authenticated DHCP refresh
    cfg.activate_device("b").unwrap();
    assert_eq!(cfg.shared_secret, "secret-b");
    assert_eq!(cfg.devices[0].ip, "192.168.1.77");
    cfg.activate_device("a").unwrap();
    assert_eq!(cfg.shared_secret, "secret-a");
    assert_eq!(cfg.peer_ip, "192.168.1.77");
    assert_ne!(cfg.session_target(), original); // rejects stale A -> B -> A workers
    assert_eq!(cfg.devices.len(), 2);
}

#[test]
fn unpair_never_silently_routes_to_another_device() {
    let mut cfg = Config::default();
    cfg.remember_device(device("a")).unwrap();
    cfg.remember_device(device("b")).unwrap();
    cfg.forget_device("a").unwrap();
    assert_eq!(cfg.devices.len(), 1);
    assert!(cfg.active_device_id.is_empty());
    assert!(!cfg.is_complete());
    assert!(cfg.peer_addrs().is_empty());
    let loaded = Config::from_toml(&toml::to_string(&cfg).unwrap()).unwrap();
    assert!(loaded.active_device_id.is_empty());
    cfg.activate_device("b").unwrap();
    cfg.forget_device("b").unwrap();
    let loaded = Config::from_toml(&toml::to_string(&cfg).unwrap()).unwrap();
    assert!(loaded.devices.is_empty());
}

#[test]
fn forgetting_an_inactive_device_does_not_interrupt_the_active_one() {
    let mut cfg = Config::default();
    cfg.remember_device(device("a")).unwrap();
    cfg.remember_device(device("b")).unwrap();
    let target = cfg.session_target();
    cfg.forget_device("b").unwrap();
    assert_eq!(cfg.session_target(), target);
    assert!(cfg.activate_device("missing").is_err());
    assert_eq!(cfg.session_target(), target);
}

#[test]
fn repaired_identity_keeps_its_local_id_and_grants_but_invalidates_session() {
    let mut cfg = Config::default();
    let mut a = device("a");
    a.permissions.clipboard = false;
    cfg.remember_device(a).unwrap();
    let old = cfg.session_target();
    let mut repaired = device("new-random-local-id");
    repaired.peer_device_id = "identity-a".into();
    repaired.shared_secret = "rotated-key".into();
    assert_eq!(cfg.remember_device(repaired).unwrap(), "a");
    assert_eq!(cfg.devices.len(), 1);
    assert!(!cfg.permissions().clipboard);
    assert_eq!(cfg.shared_secret, "rotated-key");
    assert_ne!(cfg.session_target(), old);
}

#[test]
fn discovery_names_and_addresses_do_not_replace_another_identity() {
    let mut cfg = Config::default();
    cfg.remember_device(device("a")).unwrap();
    let mut impostor = device("b");
    impostor.host = "a.local".into();
    impostor.name = "PC a".into();
    cfg.remember_device(impostor).unwrap();
    assert_eq!(cfg.devices.len(), 2);
    assert_eq!(cfg.shared_secret, "secret-a");
    let mut collision = device("a");
    collision.peer_device_id = "foreign-id".into();
    assert!(cfg.remember_device(collision).is_err());
    assert_eq!(cfg.shared_secret, "secret-a");
}

#[test]
fn reapproving_the_same_credential_keeps_pinned_identity_and_local_permissions() {
    let mut cfg = Config::default();
    let mut original = device("a");
    original.permissions.input = false;
    cfg.remember_device(original.clone()).unwrap();
    original.id.clear();
    original.peer_device_id.clear(); // pairing v1 does not send desktop identity
    original.permissions = Default::default();
    assert_eq!(cfg.remember_device(original).unwrap(), "a");
    assert_eq!(cfg.devices.len(), 1);
    assert_eq!(cfg.peer_device_id, "identity-a");
    assert!(!cfg.permissions().input);
}

#[test]
fn unselected_registry_never_uses_stale_flat_credentials() {
    let cfg = Config {
        devices: vec![device("a")],
        shared_secret: "stale".into(),
        peer_host: "old.local".into(),
        ..Default::default()
    };
    let loaded = Config::from_toml(&toml::to_string(&cfg).unwrap()).unwrap();
    assert!(!loaded.is_complete());
    assert!(loaded.peer_addrs().is_empty());
}

#[test]
fn rejects_ambiguous_registry_and_dangling_selection() {
    let mut cfg = Config::default();
    cfg.devices = vec![device("a"), device("a")];
    assert!(Config::from_toml(&toml::to_string(&cfg).unwrap()).is_err());
    cfg.devices.pop();
    cfg.active_device_id = "missing".into();
    assert!(Config::from_toml(&toml::to_string(&cfg).unwrap()).is_err());
    cfg.active_device_id = "a".into();
    cfg.devices[0].shared_secret.clear();
    assert!(Config::from_toml(&toml::to_string(&cfg).unwrap()).is_err());
}

#[test]
fn changing_active_grants_invalidates_sessions_without_rotating_the_pairing_key() {
    let mut cfg = Config::default();
    cfg.remember_device(device("a")).unwrap();
    cfg.remember_device(device("b")).unwrap();
    let target = cfg.session_target();
    let mut grants = cfg.permissions();
    grants.input = false;
    cfg.set_device_permissions("b", grants.clone()).unwrap();
    assert_eq!(cfg.session_target(), target);
    cfg.set_device_permissions("a", grants.clone()).unwrap();
    assert_ne!(cfg.session_target(), target);
    assert_eq!(cfg.shared_secret, target.secret);
    assert!(!cfg.permissions().input);
    cfg.activate_device("b").unwrap();
    assert!(!cfg.permissions().input);
}

#[test]
fn concurrent_pairings_and_background_updates_are_persisted_without_losing_devices() {
    let dir = std::env::temp_dir().join(format!(
        "keyboard-it-registry-{}",
        protocol::secure::generate_key()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let previous = std::env::var_os("KEYBOARD_IT_CONFIG");
    std::env::set_var("KEYBOARD_IT_CONFIG", &path);
    struct Cleanup(std::path::PathBuf, Option<std::ffi::OsString>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(value) = &self.1 {
                std::env::set_var("KEYBOARD_IT_CONFIG", value);
            } else {
                std::env::remove_var("KEYBOARD_IT_CONFIG");
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir, previous);
    std::fs::write(
        &path,
        "role='sender'\nshared_secret='legacy-key'\npeer_host='original.local'\n",
    )
    .unwrap();
    let threads: Vec<_> = (0..12)
        .map(|i| {
            std::thread::spawn(move || {
                Config::transaction(|c| c.remember_device(device(&format!("pc-{i}")))).unwrap();
                Config::update(|c| c.show_stream_stats = true).unwrap();
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let cfg = Config::load().unwrap().unwrap();
    assert_eq!(cfg.devices.len(), 13);
    assert_eq!(cfg.shared_secret, "legacy-key");
    assert!(cfg.show_stream_stats);
    let before = std::fs::read(&path).unwrap();
    assert!(Config::transaction(|c| {
        c.devices.clear();
        c.activate_device("missing")
    })
    .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
