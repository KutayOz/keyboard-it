//! TCP receiver: accept connection → Noise handshake → decode events + inject.
//! Start/Stop from the Windows GUI works because the accept loop is interruptible and a
//! live connection is cut via `shutdown`, which ends the blocking read.
//! Each connection runs on its OWN thread and "newest connection wins": when the Mac
//! wakes from sleep and reconnects, the half-open dead session is dropped immediately.

use std::collections::HashSet;
use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use protocol::{InputEvent, KeyEvent, MsgType};

use crate::inject;

/// Connection state — carried from the background thread to the GUI status line (see gui.rs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnStatus {
    /// Encrypted channel established.
    Connected,
    /// Connection closed/lost; listening again.
    Disconnected,
    /// Handshake failed — most likely the pairing key differs between the two machines.
    HandshakeFailed,
}

type OnConn = Arc<dyn Fn(ConnStatus) + Send + Sync>;

/// Live connection slot: (generation, stream clone). The generation number keeps the
/// thread of an OLD connection that dies late from clearing the record of the NEW
/// connection that replaced it and from clobbering the GUI state.
type ConnSlot = Arc<Mutex<Option<(u64, TcpStream)>>>;

/// Dead peer detection: TCP keepalive. If the Mac sleeps or Wi-Fi drops (hard cut, no
/// EOF/RST), the read does not block forever; it errors within ~30 s and keys still held
/// down on Windows are released.
fn set_keepalive(stream: &TcpStream) {
    use socket2::{SockRef, TcpKeepalive};
    let ka = TcpKeepalive::new()
        .with_time(Duration::from_secs(5))
        .with_interval(Duration::from_secs(3));
    let _ = SockRef::from(stream).set_tcp_keepalive(&ka);
}

/// Clear the slot if it still holds our generation. `true` = we were the current
/// connection (reporting status is on us); `false` = Stop or a newer connection took over.
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

/// Single connection: handshake + event loop (one thread per connection).
/// The clone goes into the slot in accept_loop so `stop()` can always cut a blocking read.
fn handle_client(
    mut stream: TcpStream,
    my_gen: u64,
    psk: &[u8; 32],
    conn: &ConnSlot,
    on_conn: &OnConn,
) {
    let peer = stream.peer_addr().ok();
    let _ = stream.set_nodelay(true);
    let _ = stream.set_nonblocking(false); // handshake/reads must block
    println!("connected: {peer:?}");

    // A silent foreign connection (port scanner, `nc`, etc.) must not lock the listener
    // indefinitely: the handshake has to finish within 5 s.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let mut transport = match protocol::secure::handshake_responder(&mut stream, psk) {
        Ok(t) => {
            println!("encrypted channel established (Noise NNpsk0).");
            t
        }
        Err(e) => {
            eprintln!("handshake failed (wrong key?): {e}");
            // Timeout/EOF/reset is not a key problem (scanners etc.); carry a real
            // handshake failure to the GUI — stderr is invisible in release builds, so
            // this is the only way the user notices a wrong key.
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
    // Handshake done: drop the timeout (idling is legitimate); from here on a dead
    // connection is caught by TCP keepalive.
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
                    println!("connection closed: {peer:?}");
                } else {
                    eprintln!("read/decode error: {e}");
                }
                for hid in held.drain() {
                    inject::handle(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 });
                }
                for button in held_btns.drain() {
                    inject::handle_mouse(InputEvent::MouseButton { button, down: false });
                }
                // Only report status if we are still the current connection; if Stop or
                // a newer connection took over, do not clobber their message.
                if clear_if_current(conn, my_gen) {
                    on_conn(ConnStatus::Disconnected);
                }
                break;
            }
        }
    }
}

/// Interruptible accept loop. Returns when `stop` turns true. Each connection gets its
/// own thread and a new connection cuts the old one (newest wins) — a half-open dead
/// session can never block reconnection.
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

                // Cut the old connection + put the clone into the slot — all under the
                // lock so a race with Stop cannot leave an orphaned live connection.
                {
                    let mut slot = conn.lock().unwrap();
                    if stop.load(Ordering::Relaxed) {
                        let _ = stream.shutdown(Shutdown::Both);
                        break;
                    }
                    if let Some((_, old)) = slot.take() {
                        println!("new connection — cutting the old session");
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
                eprintln!("accept error: {e}");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// Stoppable listener handle.
pub struct Handle {
    stop: Arc<AtomicBool>,
    conn: ConnSlot,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// Stop listening: flip the flag, cut the live connection, join the accept thread
    /// (the accept loop does not block, so the join returns quickly).
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

/// Start listening. Key/port errors return IMMEDIATELY (shown in the GUI).
/// `on_conn(ConnStatus)` is called from the background thread when a connection is
/// established or lost, or when the handshake fails.
pub fn start<F: Fn(ConnStatus) + Send + Sync + 'static>(
    cfg: &protocol::config::Config,
    on_conn: F,
) -> io::Result<Handle> {
    let psk = protocol::secure::psk_from_config_or_env(cfg)?; // key error -> GUI
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?; // port error -> GUI
    listener.set_nonblocking(true)?;
    println!("win-receiver listening on 0.0.0.0:{} — waiting for connection", cfg.port);

    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    let on_conn: OnConn = Arc::new(on_conn);
    let (s, c) = (stop.clone(), conn.clone());
    let thread = thread::spawn(move || accept_loop(listener, psk, &s, &c, &on_conn));

    Ok(Handle { stop, conn, thread: Some(thread) })
}

/// Non-Windows dry-run: blocking variant (no stop; listens until the process dies).
#[cfg(not(windows))]
pub fn serve(
    cfg: &protocol::config::Config,
    on_conn: impl Fn(ConnStatus) + Send + Sync + 'static,
) -> io::Result<()> {
    println!("(no injection on this platform — incoming keys are only printed [dry-run])");
    let psk = protocol::secure::psk_from_config_or_env(cfg)?;
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;
    listener.set_nonblocking(true)?;
    println!("win-receiver listening on 0.0.0.0:{} — waiting for connection", cfg.port);
    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    let on_conn: OnConn = Arc::new(on_conn);
    accept_loop(listener, psk, &stop, &conn, &on_conn);
    Ok(())
}
