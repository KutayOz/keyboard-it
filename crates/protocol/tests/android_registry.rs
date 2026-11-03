use protocol::{
    config::Config,
    devices::{DeviceTransport, PairedDevice, Permissions, Platform},
};
fn android(serial: &str, identity: &str) -> PairedDevice {
    PairedDevice {
        transport: DeviceTransport::Adb {
            serial: serial.into(),
            android_id: identity.into(),
        },
        id: String::new(),
        name: "Android".into(),
        platform: Platform::Android,
        permissions: Permissions {
            screen: true,
            input: true,
            clipboard: false,
        },
        capabilities: None,
        peer_device_id: String::new(),
        shared_secret: String::new(),
        host: String::new(),
        ip: String::new(),
        port: 5599,
        desktop_port: 5601,
    }
}
fn pc() -> PairedDevice {
    PairedDevice {
        transport: DeviceTransport::Native,
        id: "pc".into(),
        name: "PC".into(),
        platform: Platform::Windows,
        permissions: Default::default(),
        capabilities: None,
        peer_device_id: "pc-identity".into(),
        shared_secret: "pc-secret".into(),
        host: "pc.invalid".into(),
        ip: String::new(),
        port: 5599,
        desktop_port: 5601,
    }
}
#[test]
fn android_and_native_pairings_coexist_without_sharing_credentials() {
    let mut cfg = Config::default();
    cfg.remember_device(pc()).unwrap();
    let native = cfg.session_target();
    let phone = cfg
        .remember_device(android("phone", "1234567890abcdef"))
        .unwrap();
    assert_eq!(cfg.session_target(), native);
    cfg.activate_device(&phone).unwrap();
    assert!(cfg.is_complete() && cfg.android_selected());
    assert!(cfg.shared_secret.is_empty() && cfg.peer_addrs().is_empty());
    assert!(protocol::secure::psk_from_config_or_env(&cfg).is_err());
    let loaded = Config::from_toml(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
    assert!(loaded.android_selected());
    assert_eq!(
        loaded
            .devices
            .iter()
            .find(|d| d.id == "pc")
            .unwrap()
            .shared_secret,
        "pc-secret"
    );
    cfg.activate_device("pc").unwrap();
    assert_eq!(cfg.shared_secret, "pc-secret");
    assert_eq!(cfg.peer_host, "pc.invalid");
}
#[test]
fn usb_to_wifi_reapproval_preserves_local_id_and_grants_but_invalidates_the_old_session() {
    let mut cfg = Config::default();
    let id = cfg
        .remember_device(android("usb-serial", "1234567890abcdef"))
        .unwrap();
    cfg.set_device_permissions(
        &id,
        Permissions {
            screen: true,
            input: false,
            clipboard: false,
        },
    )
    .unwrap();
    let old = cfg.session_target();
    let readded = cfg
        .remember_device(android("192.168.1.2:41234", "1234567890abcdef"))
        .unwrap();
    assert_eq!(readded, id);
    assert_eq!(cfg.devices.len(), 1);
    assert!(!cfg.permissions().input);
    assert_ne!(old, cfg.session_target());
    assert!(cfg.target_revision > old.revision);
}
#[test]
fn serial_reuse_by_a_different_phone_never_overwrites_the_pinned_identity() {
    let mut cfg = Config::default();
    let a = cfg
        .remember_device(android("same-address", "1111111111111111"))
        .unwrap();
    let old = cfg.session_target();
    let b = cfg
        .remember_device(android("same-address", "2222222222222222"))
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(cfg.devices.len(), 2);
    assert_eq!(cfg.session_target(), old);
}
#[test]
fn forgetting_the_active_phone_never_implicitly_selects_a_saved_pc() {
    let mut cfg = Config::default();
    cfg.remember_device(pc()).unwrap();
    let phone = cfg
        .remember_device(android("phone", "1234567890abcdef"))
        .unwrap();
    cfg.activate_device(&phone).unwrap();
    cfg.forget_device(&phone).unwrap();
    assert!(cfg.active_device_id.is_empty());
    assert!(cfg.shared_secret.is_empty() && cfg.peer_addrs().is_empty());
    assert_eq!(cfg.devices.len(), 1);
}
#[test]
fn malformed_android_records_and_cross_adapter_credentials_are_rejected() {
    let mut bad = android("--evil", "1234567890abcdef");
    assert!(!bad.valid_connection());
    bad = android("phone", "null");
    assert!(!bad.valid_connection());
    bad = android("phone", "1234567890abcdef");
    bad.shared_secret = "native-secret".into();
    assert!(Config::default().remember_device(bad).is_err());
    let mut cfg = Config::default();
    cfg.remember_device(android("phone", "1234567890abcdef"))
        .unwrap();
    cfg.devices[0].platform = Platform::Windows;
    assert!(Config::from_toml(&toml::to_string(&cfg).unwrap()).is_err());
}
#[test]
fn loading_an_android_selection_clears_stale_native_flat_settings() {
    let mut cfg = Config::default();
    cfg.remember_device(android("phone", "abcdef")).unwrap();
    cfg.shared_secret = "stale-secret".into();
    cfg.peer_host = "stale.invalid".into();
    let loaded = Config::from_toml(&toml::to_string(&cfg).unwrap()).unwrap();
    assert!(loaded.shared_secret.is_empty() && loaded.peer_addrs().is_empty());
}
