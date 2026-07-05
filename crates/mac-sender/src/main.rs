//! mac-sender: (M2'de) MacBook klavyesini CGEventTap ile yakalayıp tuşları
//! `protocol::KeyEvent` olarak TCP ile win-receiver'a gönderecek.
//!
//! M1 (bu sürüm): henüz yakalama YOK. Ağ yolunu kanıtlamak için, verilen adrese
//! bağlanıp sabit-kodlanmış "hello" dizisini KeyEvent olarak gönderir.
//!
//! Kullanım:
//!   cargo run -p mac-sender -- <windows-ip>[:port]
//!   (port verilmezse protocol::DEFAULT_PORT kullanılır; adres verilmezse 127.0.0.1)
//!
//! Gerçek CGEventTap yakalama iskeleti `src/capture.rs` içinde referans; M2'de
//! `mod capture;` ile etkinleştirilecek.

use std::net::TcpStream;
use std::thread::sleep;
use std::time::Duration;

use protocol::{KeyEvent, MsgType, DEFAULT_PORT};

/// Bir ASCII harfini/space'i USB HID Usage koduna çevir (Usage Page 0x07).
fn hid_for(c: char) -> Option<u16> {
    Some(match c {
        'a'..='z' => 0x04 + (c as u16 - 'a' as u16),
        ' ' => 0x2C,
        _ => return None,
    })
}

fn main() -> std::io::Result<()> {
    // Adresi argümandan al; sadece IP verilmişse portu ekle.
    let arg = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1".to_string());
    let addr = if arg.contains(':') {
        arg
    } else {
        format!("{arg}:{DEFAULT_PORT}")
    };

    println!("bağlanılıyor: {addr}");
    // win-receiver henüz ayakta değilse birkaç saniye tekrar dene.
    let mut stream = None;
    for _ in 0..40 {
        match TcpStream::connect(&addr) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => sleep(Duration::from_millis(100)),
        }
    }
    let mut stream = stream.expect("bağlanılamadı (win-receiver dinliyor mu?)");
    stream.set_nodelay(true)?;
    println!("bağlandı. 'hello' gönderiliyor...");

    for c in "hello".chars() {
        let hid = hid_for(c).expect("bu karakter için HID eşlemesi yok");
        KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }.write_framed(&mut stream)?;
        sleep(Duration::from_millis(15));
        KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }.write_framed(&mut stream)?;
        sleep(Duration::from_millis(40));
    }

    println!("gönderildi. (win-receiver tarafında 'hello' görünmeli / dry-run'da yazdırılmalı)");
    Ok(())
}
