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
use std::time::{Duration, Instant};

use protocol::pairing::{PairDecision, PairRequest};
use protocol::{InputEvent, KeyEvent, MsgType};

use crate::diag::plog;
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

/// Asks the user whether to accept a pairing request. BLOCKING by design: it is
/// expected to put a dialog on screen and wait for a click (or time out into
/// `false`). Returning `false` declines and the peer is told so.
pub type PairDecide = Arc<dyn Fn(&PairRequest) -> bool + Send + Sync>;

/// Handshake + name exchange must finish quickly; only the human decision is
/// allowed to take its time, and that happens inside the callback with no I/O.
const PAIR_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// After a decline, ignore further requests for a while so a host on the LAN
/// cannot reopen the dialog in a loop.
const PAIR_COOLDOWN: Duration = Duration::from_secs(10);

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

/// Everything the pairing listener needs to answer a request. The secret is
/// whatever the session listener is using, so a pairing hands over exactly the
/// key that will work.
struct PairCtx {
    secret: String,
    session_port: u16,
    my_name: String,
    decide: PairDecide,
    /// True while a dialog is on screen. Gates the whole feature to one prompt
    /// at a time: a second request is closed instead of queued.
    busy: Arc<AtomicBool>,
    /// Set after a decline, so a host on the LAN cannot reopen the dialog in a loop.
    cooldown_until: Arc<Mutex<Option<Instant>>>,
}

/// This machine's name, as shown in the dialog on the Mac.
fn this_device_name() -> String {
    #[cfg(windows)]
    {
        crate::netinfo::hostname()
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "keyboard-it PC".to_string())
    }
}

/// The literal pairing key to hand out. Config first, `KEYBOARD_IT_KEY` second —
/// the same precedence `psk_from_config_or_env` uses, so a pairing can never
/// hand over a key that differs from the one the session listener expects.
fn secret_to_share(cfg: &protocol::config::Config) -> String {
    if !cfg.shared_secret.is_empty() {
        return cfg.shared_secret.clone();
    }
    std::env::var("KEYBOARD_IT_KEY").unwrap_or_default()
}

/// One pairing attempt. Returns true if the user accepted.
fn handle_pair(mut stream: TcpStream, ctx: &PairCtx) -> bool {
    let peer = stream.peer_addr().ok();
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(PAIR_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PAIR_IO_TIMEOUT));

    let decide = ctx.decide.clone();
    let t0 = Instant::now();
    match protocol::pairing::pair_responder(
        &mut stream,
        &ctx.secret,
        ctx.session_port,
        &ctx.my_name,
        |req| decide(req),
    ) {
        Ok(PairDecision::Accepted(req)) => {
            plog!("paired with {:?} ({peer:?}) after {:.1}s", req.peer_name, t0.elapsed().as_secs_f32());
            true
        }
        Ok(PairDecision::Declined(req)) => {
            plog!(
                "declined {:?} ({peer:?}) after {:.1}s — a decline at ~60s is the auto-decline, not a click",
                req.peer_name,
                t0.elapsed().as_secs_f32()
            );
            false
        }
        Err(e) => {
            // Port scanners and half-open probes land here too, so this is not
            // worth surfacing in the UI — but it IS worth recording, because a
            // request that dies here never reaches `decide` and so never puts a
            // dialog on screen.
            plog!("pairing attempt from {peer:?} failed after {:.1}s: {e}", t0.elapsed().as_secs_f32());
            false
        }
    }
}

/// Interruptible pairing accept loop.
///
/// The decision callback blocks on a human, so it must NOT run here: `stop()`
/// joins this thread, and on Windows `stop()` is called from the very UI thread
/// that has to service the dialog. Handling a request inline would deadlock the
/// two against each other until the 60 s decision timeout expired. Each request
/// therefore gets a detached thread, and this loop stays responsive to `stop`
/// within ~100 ms.
fn pair_accept_loop(listener: TcpListener, ctx: Arc<PairCtx>, stop: &Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let cooling = ctx
                    .cooldown_until
                    .lock()
                    .map(|t| t.is_some_and(|t| Instant::now() < t))
                    .unwrap_or(false);
                if cooling {
                    // Both of these hang up before the handshake, so the Mac
                    // reports "the PC turned down the request" and no dialog is
                    // ever asked for. Distinguishing them from "nothing arrived"
                    // is most of the value of this file.
                    plog!("request from {peer:?} REJECTED: cooling down after a recent decline");
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                // swap, not store: whoever flips false->true owns the dialog.
                if ctx.busy.swap(true, Ordering::SeqCst) {
                    plog!("request from {peer:?} REJECTED: another request already owns the dialog");
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                plog!("request from {peer:?} accepted for handling");
                let ctx = ctx.clone();
                thread::spawn(move || {
                    let accepted = handle_pair(stream, &ctx);
                    if !accepted {
                        if let Ok(mut t) = ctx.cooldown_until.lock() {
                            *t = Some(Instant::now() + PAIR_COOLDOWN);
                        }
                    }
                    ctx.busy.store(false, Ordering::SeqCst);
                });
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                eprintln!("pairing accept error: {e}");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// Bind the pairing listener and start its thread. Returns the port it actually
/// got, so the caller can publish it over mDNS.
///
/// Failure is NON-fatal: an already-paired Mac keeps working, only new pairings
/// are unavailable. Prefers `port + 1` (a stable, documentable number for a
/// firewall rule) and falls back to an ephemeral port so a clash can never stop
/// the app from starting.
fn start_pair_listener(
    cfg: &protocol::config::Config,
    decide: PairDecide,
) -> Option<(u16, Arc<AtomicBool>, JoinHandle<()>)> {
    let secret = secret_to_share(cfg);
    if secret.is_empty() {
        plog!("pairing DISABLED: no key to hand out");
        return None;
    }
    let preferred = protocol::default_pairing_port(cfg.port);
    let listener = TcpListener::bind(("0.0.0.0", preferred))
        .or_else(|e| {
            eprintln!("pairing port {preferred} unavailable ({e}); falling back to an ephemeral port");
            TcpListener::bind(("0.0.0.0", 0))
        })
        .ok()?;
    let port = listener.local_addr().ok()?.port();
    if listener.set_nonblocking(true).is_err() {
        return None;
    }
    plog!("pairing listener on 0.0.0.0:{port} (session port {})", cfg.port);

    let ctx = Arc::new(PairCtx {
        secret,
        session_port: cfg.port,
        my_name: this_device_name(),
        decide,
        busy: Arc::new(AtomicBool::new(false)),
        cooldown_until: Arc::new(Mutex::new(None)),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let thread = thread::spawn(move || pair_accept_loop(listener, ctx, &s));
    Some((port, stop, thread))
}

/// Advertise the listener over mDNS/DNS-SD. This is the ONLY way the Mac learns
/// this PC exists — there is no manual address entry any more — so a failure
/// here means "invisible", not "degraded". Still non-fatal: an already-paired
/// Mac reconnects from its stored config regardless.
///
/// Deliberately not `#[cfg(windows)]`: the macOS dry-run has to be discoverable
/// too, or the whole pairing flow cannot be exercised on one machine.
fn advertise_mdns(port: u16, pair_port: Option<u16>) -> Option<(mdns_sd::ServiceDaemon, String)> {
    let host = this_device_name();
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("mDNS advertising unavailable: {e}");
            return None;
        }
    };
    // TXT carries what the Mac needs BEFORE it has a key: which port to pair on
    // and which pairing version this PC speaks. The SRV port stays the session
    // port, so an already-paired Mac ignores all of this.
    let mut props: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    props.insert(
        protocol::MDNS_TXT_PAIR_VERSION.to_string(),
        protocol::pairing::PAIRING_VERSION.to_string(),
    );
    if let Some(p) = pair_port {
        props.insert(protocol::MDNS_TXT_PAIR_PORT.to_string(), p.to_string());
    }

    // No explicit IP list: addr_auto lets the daemon track interface addresses itself,
    // so the advertisement stays correct when the PC switches networks while running.
    let info = match mdns_sd::ServiceInfo::new(
        protocol::MDNS_SERVICE,
        &host,
        &format!("{host}.local."),
        (),
        port,
        props,
    ) {
        Ok(i) => i.enable_addr_auto(),
        Err(e) => {
            eprintln!("mDNS service info rejected: {e}");
            let _ = daemon.shutdown();
            return None;
        }
    };
    let fullname = info.get_fullname().to_string();
    match daemon.register(info) {
        Ok(()) => Some((daemon, fullname)),
        Err(e) => {
            eprintln!("mDNS register failed: {e}");
            let _ = daemon.shutdown();
            None
        }
    }
}

/// Stoppable listener handle.
pub struct Handle {
    stop: Arc<AtomicBool>,
    conn: ConnSlot,
    thread: Option<JoinHandle<()>>,
    /// Pairing listener stop flag + thread. Stopped alongside the session
    /// listener: "Stopped" must mean not pairable either.
    pair: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    /// mDNS daemon + registered service fullname; dropped/unregistered in `stop()` so a
    /// stopped listener does not keep advertising a port nobody answers on.
    mdns: Option<(mdns_sd::ServiceDaemon, String)>,
}

impl Handle {
    /// Stop listening: withdraw the mDNS advertisement, flip the flag, cut the live
    /// connection, join the accept thread (the accept loop does not block, so the join
    /// returns quickly).
    pub fn stop(&mut self) {
        if let Some((daemon, fullname)) = self.mdns.take() {
            // Wait briefly for the unregister so the "goodbye" packets actually leave
            // before the daemon is shut down; ignore errors — stopping must not fail.
            if let Ok(rx) = daemon.unregister(&fullname) {
                let _ = rx.recv_timeout(Duration::from_secs(1));
            }
            let _ = daemon.shutdown();
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some((_, s)) = self.conn.lock().unwrap().take() {
            let _ = s.shutdown(Shutdown::Both);
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        // Safe to join: the accept loop never runs a dialog itself (those get
        // their own detached threads), so it returns within one poll interval.
        if let Some((stop, thread)) = self.pair.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
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
/// `decide` answers pairing requests — see [`PairDecide`].
pub fn start<F: Fn(ConnStatus) + Send + Sync + 'static>(
    cfg: &protocol::config::Config,
    on_conn: F,
    decide: PairDecide,
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

    // Non-fatal: without it, already-paired Macs still connect.
    let pair = start_pair_listener(cfg, decide);
    let pair_port = pair.as_ref().map(|(p, _, _)| *p);

    Ok(Handle {
        stop,
        conn,
        thread: Some(thread),
        pair: pair.map(|(_, stop, thread)| (stop, thread)),
        mdns: advertise_mdns(cfg.port, pair_port),
    })
}

/// Non-Windows dry-run: blocking variant (no stop; listens until the process dies).
/// Pairing requests are auto-accepted after printing the code — there is no UI to
/// confirm in, and this path exists to exercise the network end to end.
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

    let decide: PairDecide = Arc::new(|req: &PairRequest| {
        println!(
            "[dry-run] pairing request from {:?}, code {} — auto-accepting",
            req.peer_name,
            protocol::pairing::code_display(&req.code)
        );
        true
    });
    // Held for the lifetime of the process: dropping it would stop the loop.
    let _pair = start_pair_listener(cfg, decide);
    let _mdns = advertise_mdns(cfg.port, _pair.as_ref().map(|(p, _, _)| *p));

    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    let on_conn: OnConn = Arc::new(on_conn);
    accept_loop(listener, psk, &stop, &conn, &on_conn);
    Ok(())
}
