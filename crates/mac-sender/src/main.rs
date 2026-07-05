//! mac-sender: MacBook klavyesini yakalayıp tuşları TCP ile win-receiver'a gönderir.
//!
//! Kullanım:
//!   cargo run -p mac-sender -- <windows-ip>[:port]     # M2: gerçek klavye yakalama (macOS)
//!   cargo run -p mac-sender -- --hello <windows-ip>    # test: sabit 'hello' gönder
//!
//! Port verilmezse protocol::DEFAULT_PORT kullanılır.

mod net;

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod keymap;

use std::io;

use protocol::{InputEvent, KeyEvent, MsgType, DEFAULT_PORT};

fn normalize_addr(arg: &str) -> String {
    if arg.contains(':') {
        arg.to_string()
    } else {
        format!("{arg}:{DEFAULT_PORT}")
    }
}

// --- Son kullanılan adresi hatırla (~/.keyboard-it-ip) ---
fn config_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".keyboard-it-ip"))
}

fn save_ip(addr: &str) {
    if let Some(p) = config_path() {
        let _ = std::fs::write(p, addr);
    }
}

fn load_ip() -> Option<String> {
    let s = std::fs::read_to_string(config_path()?).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Adresi çöz: argüman verildiyse onu kullan + hatırla; yoksa kayıtlıyı kullan.
fn resolve_addr(explicit: Option<String>) -> String {
    match explicit {
        Some(a) => {
            let addr = normalize_addr(&a);
            save_ip(&addr); // sonraki sefer argümansız çalışsın diye
            addr
        }
        None => match load_ip() {
            Some(saved) => {
                println!("kayıtlı adres kullanılıyor: {saved}  (değiştirmek için: -- <yeni-ip>)");
                saved
            }
            None => normalize_addr("127.0.0.1"),
        },
    }
}

/// Test modu: bağlan ve sabit "hello" gönder (M1 davranışı). İzin gerektirmez.
fn send_hello(addr: &str) -> io::Result<()> {
    use std::thread::sleep;
    use std::time::Duration;

    let psk = protocol::secure::psk_from_env()?;
    println!("bağlanılıyor: {addr}  (hello testi)");
    let mut stream = net::connect_retry(addr)?;
    let mut t = protocol::secure::handshake_initiator(&mut stream, &psk)?;
    println!("bağlandı (şifreli). 'hello' gönderiliyor...");
    for c in "hello".chars() {
        let hid = 0x04 + (c as u16 - 'a' as u16); // a-z -> HID usage
        protocol::secure::send_event(&mut t, &mut stream,
            &InputEvent::Key(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }))?;
        sleep(Duration::from_millis(15));
        protocol::secure::send_event(&mut t, &mut stream,
            &InputEvent::Key(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }))?;
        sleep(Duration::from_millis(40));
    }
    println!("gönderildi.");
    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (hello_mode, addr_arg) = match args.get(1).map(String::as_str) {
        Some("--hello") => (true, args.get(2).cloned()),
        Some(a) => (false, Some(a.to_string())),
        None => (false, None),
    };
    let addr = resolve_addr(addr_arg);

    if hello_mode {
        return send_hello(&addr);
    }

    #[cfg(target_os = "macos")]
    {
        capture::run(addr)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = addr;
        eprintln!("Gerçek klavye yakalama yalnızca macOS'ta. Test için: mac-sender -- --hello <ip>");
        Ok(())
    }
}
