//! win-receiver: TCP'den `protocol::KeyEvent` alıp Windows'a `SendInput` ile basar.
//!
//! M1 (bu sürüm): bir TCP portu dinler, gelen framed tuş olaylarını çözer,
//! HID usage -> Windows scancode çevirir ve enjekte eder.
//!
//! Çapraz platform: dinleme + çözme her OS'ta çalışır. Enjeksiyon Windows'a özeldir;
//! Windows DIŞINDA (ör. senin Mac'inde) "dry-run" modunda çalışır — gelen tuşları
//! enjekte etmeden yazdırır. Böylece tüm ağ/protokol yolu Windows olmadan test edilir.

use std::io;
use std::net::TcpListener;

use protocol::{KeyEvent, DEFAULT_PORT};

mod inject;
mod scancode;

fn main() -> io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", DEFAULT_PORT))?;
    println!("win-receiver dinliyor: 0.0.0.0:{DEFAULT_PORT} — bağlantı bekleniyor");
    #[cfg(not(windows))]
    println!("(bu platformda enjeksiyon YOK — gelen tuşlar sadece yazdırılır [dry-run])");

    // Basit v1: aynı anda tek bağlantı. Bağlantı düşerse yeni bağlantı bekle.
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("bağlantı kabul hatası: {e}");
                continue;
            }
        };
        let peer = stream.peer_addr().ok();
        let _ = stream.set_nodelay(true);
        println!("bağlandı: {peer:?}");

        loop {
            match KeyEvent::read_framed(&mut stream) {
                Ok(ev) => inject::handle(ev),
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    println!("bağlantı kapandı: {peer:?}");
                    // TODO(M3+): burada basılı kalan tüm tuşları serbest bırak (all-keys-up).
                    break;
                }
                Err(e) => {
                    eprintln!("okuma/çözme hatası: {e}");
                    break;
                }
            }
        }
    }
    Ok(())
}
