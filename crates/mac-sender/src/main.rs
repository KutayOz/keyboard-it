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
    // Eksik/bozuk config'te ÇIKMA: dağıtılan .app LSUIElement olduğundan stderr
    // görünmez ve uygulama 'hiç açılmıyor' sanılırdı (bulgu düzeltmesi). Bunun
    // yerine menü çubuğu yine kurulur, başlık 'Ayar gerekli' olur ve capture::run
    // ilk açılışta kullanıcıyı diyalogla 'Ayarlar...'a yönlendirir.
    let mut cfg = match protocol::config::Config::load() {
        Ok(Some(c)) => c,
        Ok(None) => {
            // İlk çalıştırma: 'Ayarlar...'ın açacağı varsayılan şablonu hemen oluştur.
            let c = protocol::config::Config { role: Role::Sender, ..Default::default() };
            let _ = c.save();
            c
        }
        Err(e) => {
            eprintln!("config okunamadı ({e}) — varsayılanla devam ediliyor; 'Ayarlar...' ile düzelt.");
            protocol::config::Config::default()
        }
    };
    cfg.role = Role::Sender;
    if let Some(ip) = ip_arg {
        cfg.peer_host = ip.clone();
        let _ = cfg.save(); // sonraki sefer argümansız çalışsın
    }
    if cfg.peer_host.is_empty() {
        // Bilgi amaçlı (terminalden çalıştıranlar için); AKIŞI KESMEZ.
        eprintln!(
            "peer_host ayarlı değil — menü çubuğu 'Ayar gerekli' gösterecek. Windows IP/host'unu\n  \
             cargo run -p mac-sender -- <ip-veya-host>  ile ya da 'Ayarlar...' menüsünden ver."
        );
    }

    if hello_mode {
        if cfg.peer_host.is_empty() {
            eprintln!("--hello için adres gerekli:  cargo run -p mac-sender -- --hello <ip>");
            return Ok(());
        }
        return send_hello(&cfg);
    }

    #[cfg(target_os = "macos")]
    {
        // Tek örnek: LaunchAgent (oturum açılışı) + elle açılış çakışmasın diye
        // sabit bir loopback portunu kilit olarak tut. Zaten bağlıysa 2. örnek çıkar.
        // (_guard, capture::run hiç dönmediği için süreç ömrü boyunca canlı kalır.)
        let _guard = match std::net::TcpListener::bind(("127.0.0.1", 5598)) {
            Ok(l) => l,
            Err(_) => {
                eprintln!("keyboard-it zaten çalışıyor (menü çubuğuna bak).");
                return Ok(());
            }
        };
        capture::run(cfg)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cfg;
        eprintln!("Gerçek klavye yakalama yalnızca macOS. Test için: -- --hello <ip>");
        Ok(())
    }
}
