use super::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct Fake {
    responses: Arc<Mutex<VecDeque<Output>>>,
    calls: Arc<Mutex<Vec<CommandSpec>>>,
}
impl Fake {
    fn new(responses: impl IntoIterator<Item = Output>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            calls: Arc::default(),
        }
    }
}
impl Runner for Fake {
    fn run(&self, command: &CommandSpec, _: Duration) -> io::Result<Output> {
        self.calls.lock().unwrap().push(command.clone());
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected Android command"))
    }
}
fn output(text: &str) -> Output {
    Output {
        success: true,
        stdout: text.as_bytes().into(),
        stderr: Vec::new(),
    }
}
fn tools() -> Tools {
    Tools {
        adb: "/fixture/adb".into(),
        scrcpy: Some("/fixture/scrcpy".into()),
    }
}
fn device() -> PairedDevice {
    PairedDevice {
        transport: DeviceTransport::Adb {
            serial: "fixture-phone".into(),
            android_id: "1234567890abcdef".into(),
        },
        id: "phone".into(),
        name: "Fixture phone".into(),
        platform: Platform::Android,
        capabilities: None,
        permissions: Permissions {
            screen: true,
            input: true,
            clipboard: false,
        },
        shared_secret: String::new(),
        host: String::new(),
        ip: String::new(),
        peer_device_id: String::new(),
        port: 5599,
        desktop_port: 5601,
    }
}
const DEVICES: &str = "List of devices attached\nfixture-phone device product:test model:Test_Phone device:test transport_id:1\n";
const HELP: &str = "--serial --no-control --no-audio --no-clipboard-autosync --video-codec --window-title --max-fps --max-size --video-bit-rate";
#[test]
fn parses_multiple_devices_and_keeps_authorization_states_separate() {
    let list = parse_devices(&format!("* daemon started successfully\n{DEVICES}192.168.1.2:40123 unauthorized\nold offline\nother no permissions\n")).unwrap();
    assert_eq!(list.len(), 4);
    assert_eq!(list[0].name, "Test Phone");
    assert_eq!(list[0].state, DeviceState::Ready);
    assert_eq!(list[1].state, DeviceState::Unauthorized);
    assert_eq!(list[2].state, DeviceState::Offline);
    assert!(matches!(list[3].state, DeviceState::Unavailable(_)));
}
#[test]
fn malformed_or_ambiguous_discovery_fails_closed() {
    for text in [
        "failed to connect",
        "",
        "List of devices attached\nphone\n",
        "List of devices attached\n--evil device\n",
        "List of devices attached\na device\na device\n",
    ] {
        assert!(parse_devices(text).is_err(), "{text}");
    }
    assert!(parse_devices("List of devices attached\n")
        .unwrap()
        .is_empty());
    assert!(parse_devices("List of devices attached\na;touch device\n").is_err());
}
#[test]
fn endpoints_reject_flags_shell_syntax_and_invalid_ports() {
    for endpoint in [
        "--tcpip",
        "192.168.0.1:0",
        "host:5555",
        "0.0.0.0:5555",
        "224.0.0.1:5555",
        "127.0.0.1:4;touch",
        "127.0.0.1:4\n--evil",
    ] {
        assert!(endpoint_address(endpoint).is_err());
    }
    assert_eq!(endpoint_address("[::1]:5555").unwrap(), "[::1]:5555");
}
#[test]
fn pairing_keeps_the_code_out_of_arguments_debug_output_and_errors() {
    let fake = Fake::new([Output {
        success: false,
        stdout: b"failed code 482913".into(),
        stderr: b"482913 refused".into(),
    }]);
    let adapter = Adapter::new(tools(), fake.clone());
    let error = adapter
        .pair_wifi("127.0.0.1:43210", "482913")
        .unwrap_err()
        .to_string();
    assert!(!error.contains("482913"));
    let calls = fake.calls.lock().unwrap();
    let call = &calls[0];
    assert_eq!(call.args, ["pair", "127.0.0.1:43210"].map(OsString::from));
    assert_eq!(call.input.as_deref(), Some(b"482913\n".as_slice()));
    assert!(!format!("{call:?}").contains("482913"));
}
#[test]
fn wireless_success_requires_a_positive_tool_response_even_with_zero_exit_status() {
    let fake = Fake::new([
        output("Enter pairing code: Successfully paired to 127.0.0.1:43210 [guid=x]\n"),
        output("failed to connect to 127.0.0.1:5555\n"),
        output("already connected to 127.0.0.1:5555\n"),
    ]);
    let adapter = Adapter::new(tools(), fake);
    adapter.pair_wifi("127.0.0.1:43210", "123456").unwrap();
    assert!(adapter.connect_wifi("127.0.0.1:5555").is_err());
    adapter.connect_wifi("127.0.0.1:5555").unwrap();
}
#[test]
fn invalid_pairing_codes_never_start_a_process() {
    let fake = Fake::new([]);
    let adapter = Adapter::new(tools(), fake.clone());
    for code in ["12345", "1234567", "123;56", "１２３４５６"] {
        assert!(adapter.pair_wifi("127.0.0.1:5555", code).is_err());
    }
    assert!(fake.calls.lock().unwrap().is_empty());
}
#[test]
fn saving_rechecks_readiness_and_never_creates_native_credentials() {
    let fake = Fake::new([output(DEVICES), output("1234567890abcdef\n")]);
    let adapter = Adapter::new(tools(), fake.clone());
    let discovered = parse_devices(DEVICES).unwrap().remove(0);
    let saved = adapter.saveable(&discovered).unwrap();
    assert!(saved.is_android() && saved.valid_connection());
    assert!(
        saved.shared_secret.is_empty() && saved.host.is_empty() && !saved.permissions.clipboard
    );
    assert_eq!(
        fake.calls.lock().unwrap()[1].args,
        [
            "-s",
            "fixture-phone",
            "shell",
            "settings",
            "get",
            "secure",
            "android_id"
        ]
        .map(OsString::from)
    );
    let adapter = Adapter::new(
        tools(),
        Fake::new([output(
            "List of devices attached\nfixture-phone unauthorized\n",
        )]),
    );
    assert!(adapter.saveable(&discovered).is_err());
}
#[test]
fn unauthorized_devices_are_not_queried_or_saved() {
    let fake = Fake::new([]);
    let adapter = Adapter::new(tools(), fake.clone());
    let unavailable = AndroidDevice {
        serial: "fixture-phone".into(),
        name: "Phone".into(),
        state: DeviceState::Unauthorized,
    };
    assert!(adapter.saveable(&unavailable).is_err());
    assert!(fake.calls.lock().unwrap().is_empty());
}
#[test]
fn screen_and_input_grants_are_enforced_in_the_exact_serial_launch() {
    let mut target = device();
    target.name = "Phone $(touch /tmp/not-a-command)".into();
    target.permissions.input = false;
    let command = mirror_command(&tools(), &target, StreamSettings::PROFILES[2].1).unwrap();
    assert!(command.args.contains(&"--serial=fixture-phone".into()));
    assert!(command.args.contains(&"--no-control".into()));
    assert!(
        command.args.contains(&"--no-audio".into())
            && command.args.contains(&"--no-clipboard-autosync".into())
    );
    assert!(
        command.args.contains(&"--max-size=1920".into())
            && command.args.contains(&"--video-bit-rate=20000000".into())
    );
    assert!(command
        .args
        .iter()
        .any(|s| s == "--window-title=keyboard-it · Phone $(touch /tmp/not-a-command)"));
    assert_eq!(command.env[0], ("ADB".into(), "/fixture/adb".into()));
    target.permissions.screen = false;
    assert!(mirror_command(&tools(), &target, Default::default()).is_err());
}
#[test]
fn changed_identity_or_missing_capabilities_never_launch_the_viewer() {
    let fake = Fake::new([output(HELP), output(DEVICES), output("9999999999999999\n")]);
    let error = Adapter::new(tools(), fake)
        .mirror_command(&device(), Default::default())
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    let fake = Fake::new([output("old scrcpy --serial")]);
    let adapter = Adapter::new(tools(), fake.clone());
    assert!(adapter
        .mirror_command(&device(), Default::default())
        .is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}
#[test]
fn approved_target_uses_the_same_pinned_identity_for_the_launch_preflight() {
    let fake = Fake::new([output(HELP), output(DEVICES), output("1234567890abcdef\n")]);
    let command = Adapter::new(tools(), fake)
        .mirror_command(&device(), Default::default())
        .unwrap();
    assert_eq!(command.program, PathBuf::from("/fixture/scrcpy"));
    assert!(command.args.contains(&"--serial=fixture-phone".into()));
}
#[test]
fn missing_mirror_tool_fails_before_querying_devices() {
    let mut tools = tools();
    tools.scrcpy = None;
    let fake = Fake::new([]);
    let adapter = Adapter::new(tools, fake.clone());
    assert_eq!(
        adapter
            .mirror_command(&device(), Default::default())
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(fake.calls.lock().unwrap().is_empty());
}
#[test]
fn cancelled_launches_cannot_publish_after_returning_to_the_same_device() {
    let sessions = session::Sessions::default();
    let a = sessions.begin();
    sessions.cancel();
    let b = sessions.begin();
    sessions.cancel();
    let new_a = sessions.begin();
    let command = CommandSpec::new("/nonexistent/viewer", ["--serial=fixture-phone"]);
    assert!(!sessions.launch(a, &command).unwrap());
    assert!(!sessions.launch(b, &command).unwrap());
    sessions.fail(a, "stale failure");
    assert!(sessions.status().starting && sessions.status().generation == new_a);
    assert!(!sessions.status().message.contains("stale failure"));
    sessions.cancel();
    assert!(!sessions.status().starting && !sessions.status().running);
}
#[cfg(unix)]
#[test]
fn real_tool_processes_have_bounded_output_and_timeouts_without_android() {
    use process::Runner;
    let runner = ProcessRunner;
    let echo = CommandSpec::new(
        "/bin/sh",
        [
            "-c",
            "printf '%s' \"$1\"",
            "fixture",
            "literal;$(not-executed)",
        ],
    );
    assert_eq!(
        runner.run(&echo, Duration::from_secs(2)).unwrap().stdout,
        b"literal;$(not-executed)"
    );
    let slow = CommandSpec::new("/bin/sh", ["-c", "exec sleep 5"]);
    assert_eq!(
        runner
            .run(&slow, Duration::from_millis(80))
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::TimedOut
    );
    let large = CommandSpec::new("/bin/sh", ["-c", "head -c 1100000 /dev/zero"]);
    assert!(runner.run(&large, Duration::from_secs(2)).is_err());
}
#[cfg(unix)]
#[test]
fn owned_viewer_is_reaped_on_cancellation_and_launch_errors_end_starting_state() {
    let sessions = session::Sessions::default();
    let ticket = sessions.begin();
    let command = CommandSpec::new("/bin/sh", ["-c", "exec sleep 5"]);
    assert!(sessions.launch(ticket, &command).unwrap());
    assert!(sessions.status().running);
    sessions.cancel();
    assert!(!sessions.status().running);
    let ticket = sessions.begin();
    let bad = CommandSpec::new("/does/not/exist", ["--serial=fixture-phone"]);
    let error = sessions.launch(ticket, &bad).unwrap_err();
    sessions.fail(ticket, &error.to_string());
    assert!(!sessions.status().running && !sessions.status().starting);
}

#[cfg(unix)]
#[test]
fn an_old_watchers_cleanup_cannot_close_the_new_viewer() {
    let sessions = session::Sessions::default();
    let old = sessions.begin();
    sessions.cancel();
    let current = sessions.begin();
    let command = CommandSpec::new("/bin/sh", ["-c", "exec sleep 5"]);
    assert!(sessions.launch(current, &command).unwrap());
    assert!(!sessions.cancel_if_current(old));
    assert!(sessions.status().running);
    assert!(sessions.cancel_if_current(current));
    assert!(!sessions.status().running);
}
#[cfg(unix)]
#[test]
fn viewer_failures_are_reported_separately_from_a_normal_window_close() {
    use std::{thread, time::Instant};
    let sessions = session::Sessions::default();
    for (script, failed) in [
        ("printf 'fixture failure' >&2; exit 7", true),
        ("exit 0", false),
    ] {
        let ticket = sessions.begin();
        sessions
            .launch(ticket, &CommandSpec::new("/bin/sh", ["-c", script]))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            let status = sessions.status();
            if !status.running {
                break status;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.failed, failed);
        if failed {
            assert!(status.message.contains("fixture failure"));
        }
    }
}
