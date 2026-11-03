use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

#[test]
fn console_dry_run_stops_cleanly_and_access_commands_refuse_a_live_policy_owner() {
    let dir = std::env::temp_dir().join(format!(
        "keyboard-it-console-{}",
        protocol::secure::generate_key()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("host.toml");
    std::fs::write(&path, "role = 'receiver'\nport = 0\ndesktop_port = 0\n").unwrap();
    let mut host = Command::new(env!("CARGO_BIN_EXE_keyboard-it-host"))
        .args(["--dry-run", "--config"])
        .arg(&path)
        .env_remove("KEYBOARD_IT_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = host.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if line.contains("Commands: allow CODE") {
                let _ = tx.send(());
            }
        }
    });
    let ready = rx.recv_timeout(Duration::from_secs(8));
    if ready.is_err() {
        let _ = host.kill();
        panic!(
            "console host did not start: {:?}",
            host.wait_with_output().unwrap()
        );
    }
    let attempted = Command::new(env!("CARGO_BIN_EXE_keyboard-it-host"))
        .arg("--config")
        .arg(&path)
        .arg("--revoke-all")
        .env_remove("KEYBOARD_IT_KEY")
        .output()
        .unwrap();
    assert!(!attempted.status.success());
    assert!(String::from_utf8_lossy(&attempted.stderr).contains("already in use"));
    host.stdin.as_mut().unwrap().write_all(b"quit\n").unwrap();
    assert!(host.wait().unwrap().success());
    reader.join().unwrap();
    let list = Command::new(env!("CARGO_BIN_EXE_keyboard-it-host"))
        .arg("--config")
        .arg(&path)
        .arg("--list-controllers")
        .env_remove("KEYBOARD_IT_KEY")
        .output()
        .unwrap();
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains("No authorized controllers"));
    std::fs::remove_dir_all(dir).unwrap();
}
