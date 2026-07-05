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

use protocol::{KeyEvent, MsgType, DEFAULT_PORT};

fn normalize_addr(arg: &str) -> String {
    if arg.contains(':') {
        arg.to_string()
    } else {
        format!("{arg}:{DEFAULT_PORT}")
    }
}

/// Test modu: bağlan ve sabit "hello" gönder (M1 davranışı). İzin gerektirmez.
fn send_hello(addr: &str) -> io::Result<()> {
    use std::thread::sleep;
    use std::time::Duration;

    println!("bağlanılıyor: {addr}  (hello testi)");
    let mut stream = net::connect_retry(addr)?;
    println!("bağlandı. 'hello' gönderiliyor...");
    for c in "hello".chars() {
        let hid = 0x04 + (c as u16 - 'a' as u16); // a-z -> HID usage
        KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }.write_framed(&mut stream)?;
        sleep(Duration::from_millis(15));
        KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }.write_framed(&mut stream)?;
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
    let addr = normalize_addr(&addr_arg.unwrap_or_else(|| "127.0.0.1".to_string()));

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
