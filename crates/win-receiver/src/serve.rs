//! TCP alıcı: bağlantı kabul → Noise handshake → olayları çöz + enjekte.
//! Windows GUI'den **Başlat/Durdur** edilebilir: accept döngüsü kesintilidir ve
//! canlı bağlantı `shutdown` ile kesilerek bloklayan okuma sonlandırılır.

use std::collections::HashSet;
use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use protocol::{InputEvent, KeyEvent, MsgType};

use crate::inject;

type ConnSlot = Arc<Mutex<Option<TcpStream>>>;

/// Tek bağlantı: handshake + olay döngüsü. `conn`'a canlı stream klonu konur ki
/// `stop()` dışarıdan `shutdown` ile bloklayan okumayı kesebilsin.
fn handle_client(
    mut stream: TcpStream,
    psk: &[u8; 32],
    conn: &ConnSlot,
    on_conn: &(dyn Fn(bool) + Send + Sync),
) {
    let peer = stream.peer_addr().ok();
    let _ = stream.set_nodelay(true);
    let _ = stream.set_nonblocking(false); // handshake/okuma bloklamalı
    println!("bağlandı: {peer:?}");

    // Klonu handshake'ten ÖNCE koy: Stop handshake sırasında da kesebilsin.
    if let Ok(c) = stream.try_clone() {
        *conn.lock().unwrap() = Some(c);
    }

    let mut transport = match protocol::secure::handshake_responder(&mut stream, psk) {
        Ok(t) => {
            println!("şifreli kanal kuruldu (Noise NNpsk0).");
            t
        }
        Err(e) => {
            eprintln!("el sıkışma başarısız (yanlış anahtar?): {e}");
            *conn.lock().unwrap() = None;
            return;
        }
    };
    on_conn(true);

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
                InputEvent::MouseMove { .. } | InputEvent::Scroll { .. } => inject::handle_mouse(ev),
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
                *conn.lock().unwrap() = None;
                on_conn(false);
                break;
            }
        }
    }
}

/// Kesintili accept döngüsü. `stop` true olunca döner.
fn accept_loop(
    listener: TcpListener,
    psk: [u8; 32],
    stop: &Arc<AtomicBool>,
    conn: &ConnSlot,
    on_conn: &(dyn Fn(bool) + Send + Sync),
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                handle_client(stream, &psk, conn, on_conn);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                eprintln!("bağlantı kabul hatası: {e}");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// Durdurulabilir dinleyici tutamacı.
pub struct Handle {
    stop: Arc<AtomicBool>,
    conn: ConnSlot,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// Dinlemeyi durdur: bayrağı çevir, canlı bağlantıyı kes, thread'i join et.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self.conn.lock().unwrap().take() {
            let _ = s.shutdown(Shutdown::Both);
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Dinlemeyi başlat. Anahtar/port hataları HEMEN döner (GUI'ye gösterilir).
/// `on_conn(true/false)` bağlantı kurulunca/kopunca arka thread'den çağrılır.
pub fn start<F: Fn(bool) + Send + Sync + 'static>(
    cfg: &protocol::config::Config,
    on_conn: F,
) -> io::Result<Handle> {
    let psk = protocol::secure::psk_from_config_or_env(cfg)?; // anahtar hatası -> GUI
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?; // port hatası -> GUI
    listener.set_nonblocking(true)?;
    println!("win-receiver dinliyor: 0.0.0.0:{} — bağlantı bekleniyor", cfg.port);

    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    let (s, c) = (stop.clone(), conn.clone());
    let thread = thread::spawn(move || accept_loop(listener, psk, &s, &c, &on_conn));

    Ok(Handle { stop, conn, thread: Some(thread) })
}

/// Non-Windows dry-run: bloklayan sürüm (durdurma yok; süreç ölene dek dinler).
#[cfg(not(windows))]
pub fn serve(cfg: &protocol::config::Config, on_conn: impl Fn(bool) + Send + Sync) -> io::Result<()> {
    println!("(bu platformda enjeksiyon YOK — gelen tuşlar sadece yazdırılır [dry-run])");
    let psk = protocol::secure::psk_from_config_or_env(cfg)?;
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;
    listener.set_nonblocking(true)?;
    println!("win-receiver dinliyor: 0.0.0.0:{} — bağlantı bekleniyor", cfg.port);
    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    accept_loop(listener, psk, &stop, &conn, &on_conn);
    Ok(())
}
