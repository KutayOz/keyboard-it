//! mac-sender: MacBook klavyesini yakalayıp tuşları TCP ile win-receiver'a gönderir.
//!
//! Ayarlar artık config dosyasında (protocol::config). Paylaşılan sır config'te yoksa
//! KEYBOARD_IT_KEY env var'ına düşülür (geriye-uyum). Windows host'u ilk sefer argümanla
//! verilir ve config'e kaydedilir; sonraki çalıştırmalar argümansız.
//!
//! Kullanım:
//!   cargo run -p mac-sender                  # config'teki peer_host'a bağlan (gerçek yakalama)
//!   cargo run -p mac-sender -- <ip|host>     # peer_host'u ayarla+kaydet, sonra yakala
//!   cargo run -p mac-sender -- --hello <ip>  # test: sabit 'hello' gönder

mod net;

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod keymap;
#[cfg(target_os = "macos")]
mod menubar;

use std::io;

use protocol::config::{Config, Role};
use protocol::{InputEvent, KeyEvent, MsgType};

/// Test modu: bağlan ve sabit "hello" gönder. Config'ten (ya da env yedeği) PSK alır.
fn send_hello(cfg: &Config) -> io::Result<()> {
    use std::thread::sleep;
    use std::time::Duration;

    let psk = protocol::secure::psk_from_config_or_env(cfg)?;
    let addr = cfg.peer_addr();
    println!("bağlanılıyor: {addr}  (hello testi)");
    let mut stream = net::connect_retry(&addr)?;
    let mut t = protocol::secure::handshake_initiator(&mut stream, &psk)?;
    println!("bağlandı (şifreli). 'hello' gönderiliyor...");
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
    println!("gönderildi.");
    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let hello_mode = args.get(1).map(String::as_str) == Some("--hello");
    let ip_arg = if hello_mode { args.get(2) } else { args.get(1) };

    // Config: kaynak-of-truth. CLI ile verilen ip config'i günceller (eski davranış).
    let mut cfg = protocol::config::Config::load()?.unwrap_or_default();
    cfg.role = Role::Sender;
    if let Some(ip) = ip_arg {
        cfg.peer_host = ip.clone();
        let _ = cfg.save(); // sonraki sefer argümansız çalışsın
    }
    if cfg.peer_host.is_empty() {
        eprintln!(
            "peer_host ayarlı değil. Windows IP/host'unu bir kez ver:\n  \
             cargo run -p mac-sender -- <ip-veya-host>"
        );
        return Ok(());
    }

    if hello_mode {
        return send_hello(&cfg);
    }

    #[cfg(target_os = "macos")]
    {
        capture::run(cfg)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cfg;
        eprintln!("Gerçek klavye yakalama yalnızca macOS. Test için: -- --hello <ip>");
        Ok(())
    }
}
