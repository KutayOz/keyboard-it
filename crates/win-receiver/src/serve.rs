//! TCP alıcı: bağlantı kabul → Noise handshake → olayları çöz + enjekte.
//! Windows GUI'den **Başlat/Durdur** edilebilir: accept döngüsü kesintilidir ve
//! canlı bağlantı `shutdown` ile kesilerek bloklayan okuma sonlandırılır.
//! Her bağlantı KENDİ thread'inde çalışır ve "en yeni bağlantı kazanır":
//! Mac uykudan dönüp yeniden bağlanınca yarı-açık ölü oturum anında düşürülür.

use std::collections::HashSet;
use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use protocol::{InputEvent, KeyEvent, MsgType};

use crate::inject;

/// Bağlantı durumu — arka thread'den GUI status satırına taşınır (bkz. gui.rs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnStatus {
    /// Şifreli kanal kuruldu.
    Connected,
    /// Bağlantı kapandı/koptu; yeniden dinleniyor.
    Disconnected,
    /// El sıkışma başarısız — büyük olasılıkla eşleşme anahtarı iki tarafta farklı.
    HandshakeFailed,
}

type OnConn = Arc<dyn Fn(ConnStatus) + Send + Sync>;

/// Canlı bağlantı yuvası: (nesil, stream klonu). Nesil numarası sayesinde geç
/// ölen ESKİ bir bağlantının thread'i, yerine geçen YENİ bağlantının kaydını
/// silemez ve GUI durumunu ezemez.
type ConnSlot = Arc<Mutex<Option<(u64, TcpStream)>>>;

/// Ölü peer algılama: TCP keepalive. Mac uyur / Wi-Fi düşerse (sert kopma,
/// EOF/RST gelmez) okuma sonsuza dek bloklanmaz; ~30 sn içinde hata döner ve
/// Windows'ta basılı kalan tuşlar bırakılır.
fn set_keepalive(stream: &TcpStream) {
    use socket2::{SockRef, TcpKeepalive};
    let ka = TcpKeepalive::new()
        .with_time(Duration::from_secs(5))
        .with_interval(Duration::from_secs(3));
    let _ = SockRef::from(stream).set_tcp_keepalive(&ka);
}

/// Yuva hâlâ bizim neslimizi tutuyorsa boşalt. `true` = biz güncel bağlantıydık
/// (durum bildirmek bize düşer); `false` = Stop ya da daha yeni bağlantı devraldı.
fn clear_if_current(conn: &ConnSlot, my_gen: u64) -> bool {
    let mut slot = conn.lock().unwrap();
    match *slot {
        Some((g, _)) if g == my_gen => {
            *slot = None;
            true
        }
        _ => false,
    }
}

/// Tek bağlantı: handshake + olay döngüsü (bağlantı başına bir thread).
/// Yuvaya klon accept_loop'ta konur ki `stop()` bloklayan okumayı hep kesebilsin.
fn handle_client(
    mut stream: TcpStream,
    my_gen: u64,
    psk: &[u8; 32],
    conn: &ConnSlot,
    on_conn: &OnConn,
) {
    let peer = stream.peer_addr().ok();
    let _ = stream.set_nodelay(true);
    let _ = stream.set_nonblocking(false); // handshake/okuma bloklamalı
    println!("bağlandı: {peer:?}");

    // Sessiz kalan yabancı bağlantı (port tarayıcı, `nc` vb.) dinleyiciyi
    // süresiz kilitleyemesin: el sıkışma 5 sn içinde bitmek zorunda.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let mut transport = match protocol::secure::handshake_responder(&mut stream, psk) {
        Ok(t) => {
            println!("şifreli kanal kuruldu (Noise NNpsk0).");
            t
        }
        Err(e) => {
            eprintln!("el sıkışma başarısız (yanlış anahtar?): {e}");
            // Zaman aşımı/EOF/reset anahtar sorunu değildir (tarayıcı vb.);
            // gerçek el sıkışma hatasını GUI'ye taşı — release'te stderr
            // görünmez, kullanıcı yanlış anahtarı ancak böyle fark eder.
            let network = matches!(
                e.kind(),
                io::ErrorKind::TimedOut
                    | io::ErrorKind::WouldBlock
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
            );
            if clear_if_current(conn, my_gen) && !network {
                on_conn(ConnStatus::HandshakeFailed);
            }
            return;
        }
    };
    // El sıkışma tamam: zaman aşımını kaldır (boşta beklemek meşru);
    // ölü bağlantıyı bundan sonra TCP keepalive yakalar.
    let _ = stream.set_read_timeout(None);
    on_conn(ConnStatus::Connected);

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
                // Yalnızca hâlâ güncel bağlantıysak durum bildir; Stop ya da
                // yeni bağlantı devraldıysa onların mesajını ezmeyelim.
                if clear_if_current(conn, my_gen) {
                    on_conn(ConnStatus::Disconnected);
                }
                break;
            }
        }
    }
}

/// Kesintili accept döngüsü. `stop` true olunca döner. Her bağlantı ayrı
/// thread'e verilir ve yeni bağlantı eskisini keser (en yeni kazanır) —
/// böylece yarı-açık ölü oturum yeniden bağlanmayı asla engelleyemez.
fn accept_loop(
    listener: TcpListener,
    psk: [u8; 32],
    stop: &Arc<AtomicBool>,
    conn: &ConnSlot,
    on_conn: &OnConn,
) {
    let mut generation: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                generation += 1;
                let my_gen = generation;
                set_keepalive(&stream);

                // Eskiyi kes + klonu yuvaya koy — hepsi kilit altında ki
                // Stop ile yarışıp sahipsiz canlı bağlantı kalmasın.
                {
                    let mut slot = conn.lock().unwrap();
                    if stop.load(Ordering::Relaxed) {
                        let _ = stream.shutdown(Shutdown::Both);
                        break;
                    }
                    if let Some((_, old)) = slot.take() {
                        println!("yeni bağlantı geldi — eski oturum kesiliyor");
                        let _ = old.shutdown(Shutdown::Both);
                    }
                    if let Ok(c) = stream.try_clone() {
                        *slot = Some((my_gen, c));
                    }
                }

                let psk = psk;
                let conn = conn.clone();
                let on_conn = on_conn.clone();
                thread::spawn(move || handle_client(stream, my_gen, &psk, &conn, &on_conn));
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
    /// Dinlemeyi durdur: bayrağı çevir, canlı bağlantıyı kes, accept thread'ini
    /// join et (accept döngüsü bloklamaz, join hızla döner).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some((_, s)) = self.conn.lock().unwrap().take() {
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
/// `on_conn(ConnStatus)` bağlantı kurulunca/kopunca/el sıkışma bozulunca
/// arka thread'den çağrılır.
pub fn start<F: Fn(ConnStatus) + Send + Sync + 'static>(
    cfg: &protocol::config::Config,
    on_conn: F,
) -> io::Result<Handle> {
    let psk = protocol::secure::psk_from_config_or_env(cfg)?; // anahtar hatası -> GUI
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?; // port hatası -> GUI
    listener.set_nonblocking(true)?;
    println!("win-receiver dinliyor: 0.0.0.0:{} — bağlantı bekleniyor", cfg.port);

    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    let on_conn: OnConn = Arc::new(on_conn);
    let (s, c) = (stop.clone(), conn.clone());
    let thread = thread::spawn(move || accept_loop(listener, psk, &s, &c, &on_conn));

    Ok(Handle { stop, conn, thread: Some(thread) })
}

/// Non-Windows dry-run: bloklayan sürüm (durdurma yok; süreç ölene dek dinler).
#[cfg(not(windows))]
pub fn serve(
    cfg: &protocol::config::Config,
    on_conn: impl Fn(ConnStatus) + Send + Sync + 'static,
) -> io::Result<()> {
    println!("(bu platformda enjeksiyon YOK — gelen tuşlar sadece yazdırılır [dry-run])");
    let psk = protocol::secure::psk_from_config_or_env(cfg)?;
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;
    listener.set_nonblocking(true)?;
    println!("win-receiver dinliyor: 0.0.0.0:{} — bağlantı bekleniyor", cfg.port);
    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    let on_conn: OnConn = Arc::new(on_conn);
    accept_loop(listener, psk, &stop, &conn, &on_conn);
    Ok(())
}
