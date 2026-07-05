//! win-receiver: TCP'den şifreli `protocol::KeyEvent` alıp Windows'a `SendInput` ile basar.
//!
//! Bağlantı Noise (NNpsk0) ile şifrelidir: accept'ten sonra responder el sıkışması yapılır,
//! sonra her tuş `recv_event` ile deşifre edilir. `KEYBOARD_IT_KEY` env var'ı ŞART; yoksa
//! ve mac-sender'daki ile AYNI değilse bağlantı reddedilir.
//!
//! Çapraz platform: dinleme + deşifre her OS'ta çalışır. Enjeksiyon Windows'a özeldir;
//! Windows DIŞINDA "dry-run" (yazdır) modunda çalışır.

use std::collections::HashSet;
use std::io;
use std::net::TcpListener;

use protocol::{KeyEvent, MsgType, DEFAULT_PORT};

mod inject;
mod scancode;

fn main() -> io::Result<()> {
    // Anahtarı EN BAŞTA türet; eksikse hiçbir şey açmadan (bind bile etmeden) dur.
    let psk = protocol::secure::psk_from_env()?;

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

        // Noise responder el sıkışması. Yanlış PSK / Noise-olmayan client temiz bir
        // kimlik reddi olarak düşürülür; sunucu sıradaki bağlantıyı bekler.
        let mut transport = match protocol::secure::handshake_responder(&mut stream, &psk) {
            Ok(t) => {
                println!("şifreli kanal kuruldu (Noise NNpsk0).");
                t
            }
            Err(e) => {
                eprintln!("el sıkışma başarısız (yanlış KEYBOARD_IT_KEY?): {e}");
                continue;
            }
        };

        // Bu bağlantıda basılı tuşları izle; kopunca hepsini bırak (stuck-key önleme).
        let mut held: HashSet<u16> = HashSet::new();
        loop {
            match protocol::secure::recv_event(&mut transport, &mut stream) {
                Ok(ev) => {
                    match ev.msg {
                        MsgType::Down | MsgType::Repeat => {
                            held.insert(ev.hid_usage);
                        }
                        MsgType::Up => {
                            held.remove(&ev.hid_usage);
                        }
                    }
                    inject::handle(ev);
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        println!("bağlantı kapandı: {peer:?}");
                    } else {
                        eprintln!("okuma/çözme hatası: {e}");
                    }
                    // Kalan basılı tuşları bırak — yoksa Windows'ta (çoğunlukla bir modifier) takılır.
                    for hid in held.drain() {
                        inject::handle(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 });
                    }
                    break;
                }
            }
        }
    }
    Ok(())
}
