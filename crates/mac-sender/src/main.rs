//! mac-sender: captures the MacBook keyboard and sends key events to win-receiver over TCP.
//!
//! Settings live in the config file (protocol::config). If the shared secret is missing from
//! the config, the KEYBOARD_IT_KEY env var is used as a fallback (backwards compatibility).
//! The Windows host is given as an argument once and saved to the config; later runs need no
//! arguments.
//!
//! Usage:
//!   cargo run -p mac-sender                  # connect to peer_host from config (real capture)
//!   cargo run -p mac-sender -- <ip|host>     # set and save peer_host, then capture
//!   cargo run -p mac-sender -- --hello <ip>  # test: send a fixed 'hello'

mod net;

#[cfg(target_os = "macos")]
mod autostart;
#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod keymap;
#[cfg(target_os = "macos")]
mod menubar;

use std::io;

use protocol::config::{Config, Role};
use protocol::{InputEvent, KeyEvent, MsgType};

/// Test mode: connect and send a fixed "hello". Takes the PSK from config (or the env fallback).
fn send_hello(cfg: &Config) -> io::Result<()> {
    use std::thread::sleep;
    use std::time::Duration;

    let psk = protocol::secure::psk_from_config_or_env(cfg)?;
    let addr = cfg.peer_addr();
    println!("connecting: {addr}  (hello test)");
    let mut stream = net::connect_retry(&addr)?;
    let mut t = protocol::secure::handshake_initiator(&mut stream, &psk)?;
    println!("connected (encrypted). sending 'hello'...");
    for c in "hello".chars() {
        let hid = 0x04 + (c as u16 - 'a' as u16); // a-z -> HID usage
        protocol::secure::send_event(
            &mut t,
            &mut stream,
            &InputEvent::Key(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }),
        )?;
        sleep(Duration::from_millis(15));
        protocol::secure::send_event(
            &mut t,
            &mut stream,
            &InputEvent::Key(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }),
        )?;
        sleep(Duration::from_millis(40));
    }
    println!("sent.");
    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let hello_mode = args.get(1).map(String::as_str) == Some("--hello");
    let ip_arg = if hello_mode { args.get(2) } else { args.get(1) };

    // Config is the source of truth. An IP given on the CLI updates the config.
    // Do not exit on a missing/broken config: the distributed .app is LSUIElement, so
    // stderr is invisible and the app would look like it never launched. Instead the
    // menu bar still comes up with a 'Setup needed' title, and capture::run points the
    // user to 'Settings...' with a dialog on first launch.
    let mut cfg = match protocol::config::Config::load() {
        Ok(Some(c)) => c,
        Ok(None) => {
            // First run: create the default template that 'Settings...' will open.
            let c = protocol::config::Config { role: Role::Sender, ..Default::default() };
            let _ = c.save();
            c
        }
        Err(e) => {
            eprintln!("failed to read config ({e}) — continuing with defaults; fix via 'Settings...'.");
            protocol::config::Config::default()
        }
    };
    cfg.role = Role::Sender;
    if let Some(ip) = ip_arg {
        cfg.peer_host = ip.clone();
        let _ = cfg.save(); // so later runs need no argument
    }
    if cfg.peer_host.is_empty() {
        // Informational for terminal runs; does not stop the flow.
        eprintln!(
            "peer_host is not set — the menu bar will show 'Setup needed'. Provide the Windows\n  \
             IP/host with  cargo run -p mac-sender -- <ip-or-host>  or via the 'Settings...' menu."
        );
    }

    if hello_mode {
        if cfg.peer_host.is_empty() {
            eprintln!("--hello needs an address:  cargo run -p mac-sender -- --hello <ip>");
            return Ok(());
        }
        return send_hello(&cfg);
    }

    #[cfg(target_os = "macos")]
    {
        // Single instance: hold a fixed loopback port as a lock so the LaunchAgent
        // (login) start and a manual start do not collide. If it is already bound, the
        // second instance exits. (_guard stays alive for the process lifetime because
        // capture::run never returns.)
        let _guard = match std::net::TcpListener::bind(("127.0.0.1", 5598)) {
            Ok(l) => l,
            Err(_) => {
                eprintln!("keyboard-it is already running (check the menu bar).");
                return Ok(());
            }
        };
        capture::run(cfg)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cfg;
        eprintln!("Real keyboard capture is macOS-only. For testing: -- --hello <ip>");
        Ok(())
    }
}
