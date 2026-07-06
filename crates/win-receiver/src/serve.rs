//! TCP alıcı döngüsü: bağlantı kabul et, Noise handshake, olayları çöz + enjekte et.
//! Windows'ta tray arka thread'inden, diğer OS'ta doğrudan (dry-run) çağrılır.
//! `on_status(true/false)` bağlantı kurulunca/kopunca çağrılır (tray göstergesi için).

use std::collections::HashSet;
use std::io;
use std::net::TcpListener;

use protocol::{InputEvent, KeyEvent, MsgType};

use crate::inject;

pub fn serve(cfg: &protocol::config::Config, on_status: impl Fn(bool)) -> io::Result<()> {
    let psk = protocol::secure::psk_from_config_or_env(cfg)?;
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;
    println!("win-receiver dinliyor: 0.0.0.0:{} — bağlantı bekleniyor", cfg.port);
    #[cfg(not(windows))]
    println!("(bu platformda enjeksiyon YOK — gelen tuşlar sadece yazdırılır [dry-run])");

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

        let mut transport = match protocol::secure::handshake_responder(&mut stream, &psk) {
            Ok(t) => {
                println!("şifreli kanal kuruldu (Noise NNpsk0).");
                t
            }
            Err(e) => {
                eprintln!("el sıkışma başarısız (yanlış anahtar?): {e}");
                continue;
            }
        };
        on_status(true); // tray: AKTIF

        // Bu bağlantıda basılı tuşları/fare butonlarını izle; kopunca hepsini bırak.
        let mut held: HashSet<u16> = HashSet::new();
        let mut held_btns: HashSet<u8> = HashSet::new();
        loop {
            match protocol::secure::recv_event(&mut transport, &mut stream) {
                Ok(ev) => match ev {
                    InputEvent::Key(ke) => {
                        match ke.msg {
                            MsgType::Down | MsgType::Repeat => {
                                held.insert(ke.hid_usage);
                            }
                            MsgType::Up => {
                                held.remove(&ke.hid_usage);
                            }
                        }
                        inject::handle(ke);
                    }
                    InputEvent::MouseButton { button, down } => {
                        if down {
                            held_btns.insert(button);
                        } else {
                            held_btns.remove(&button);
                        }
                        inject::handle_mouse(ev);
                    }
                    InputEvent::MouseMove { .. } | InputEvent::Scroll { .. } => {
                        inject::handle_mouse(ev);
                    }
                },
                Err(e) => {
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        println!("bağlantı kapandı: {peer:?}");
                    } else {
                        eprintln!("okuma/çözme hatası: {e}");
                    }
                    for hid in held.drain() {
                        inject::handle(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 });
                    }
                    for button in held_btns.drain() {
                        inject::handle_mouse(InputEvent::MouseButton { button, down: false });
                    }
                    on_status(false); // tray: PASIF
                    break;
                }
            }
        }
    }
    Ok(())
}
