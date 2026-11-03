//! TCP receiver: accept connection → Noise handshake → decode events + inject.
//! Start/Stop from the Windows GUI works because the accept loop is interruptible and a
//! live connection is cut via `shutdown`, which ends the blocking read.
//! Each connection runs on its OWN thread and "newest connection wins": when the Mac
//! wakes from sleep and reconnects, the half-open dead session is dropped immediately.

use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use protocol::access::AccessStore;
use protocol::pairing::{PairDecision, PairRequest};

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
pub enum PairingNotice {
    Saved(String),
    Failed(String),
}
pub type PairFeedback = Arc<dyn Fn(PairingNotice) + Send + Sync>;

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
    access: &Arc<AccessStore>,
    conn: &ConnSlot,
    on_conn: &OnConn,
    stop: &AtomicBool,
) {
    let peer = stream.peer_addr().ok();
    let _ = stream.set_nodelay(true);
    let _ = stream.set_nonblocking(false); // handshake/reads must block
    println!("connected: {peer:?}");

    // A silent foreign connection (port scanner, `nc`, etc.) must not lock the listener
    // indefinitely: the handshake has to finish within 5 s.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let keys = access.authentication_keys();
    let psks: Vec<_> = keys.iter().map(|k| k.key).collect();
    let (mut transport, key_index, _) =
        match protocol::secure::handshake_responder_many(&mut stream, &psks, 0) {
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
                if conn.lock().unwrap().is_none() && !inject::connected() && !network {
                    on_conn(ConnStatus::HandshakeFailed);
                }
                return;
            }
        };
    // NNpsk0's first flight can be replayed without knowing the PSK. Only a
    // transport record proves possession of this session's fresh DH key. Do not
    // replace a live controller (or release its held input) before that proof.
    let first_event = match protocol::secure::recv_event(&mut transport, &mut stream) {
        Ok(event) => event,
        Err(_) => return,
    };
    // Authenticated idle connections are legitimate; keepalive detects dead peers.
    let _ = stream.set_read_timeout(None);
    let authorized = access.attach_with(&keys[key_index], &stream, || {
        let mut slot = conn.lock().unwrap();
        if stop.load(Ordering::SeqCst) {
            return Err(io::Error::other("Listener stopped"));
        }
        if let Some((_, old)) = slot.take() {
            let _ = old.shutdown(Shutdown::Both);
        }
        let clone = stream.try_clone()?;
        *slot = Some((my_gen, clone));
        Ok(inject::acquire())
    });
    let Ok((lease, owner)) = authorized else {
        return;
    };
    on_conn(ConnStatus::Connected);
    let mut first = Some(first_event);
    while let Some(ev) = first.take().or_else(|| {
        protocol::secure::recv_event(&mut transport, &mut stream).ok()
    }) {
        if !lease.dispatch(|| inject::event(owner, ev)).unwrap_or(false) {
            break;
        }
    }
    if clear_if_current(conn, my_gen) && inject::owned(owner) {
        on_conn(ConnStatus::Disconnected);
    }
    inject::retire(owner);
}

/// Interruptible accept loop. Returns when `stop` turns true. Each connection gets its
/// own thread and a new connection cuts the old one (newest wins) — a half-open dead
/// session can never block reconnection.
fn accept_loop(
    listener: TcpListener,
    access: Arc<AccessStore>,
    stop: &Arc<AtomicBool>,
    conn: &ConnSlot,
    on_conn: &OnConn,
) {
    let mut generation: u64 = 0;
    let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                generation += 1;
                let my_gen = generation;
                set_keepalive(&stream);

                if pending.fetch_add(1, Ordering::SeqCst) >= 4 {
                    pending.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let conn = conn.clone();
                let on_conn = on_conn.clone();
                let stop = stop.clone();
                let pending = pending.clone();
                let access = access.clone();
                thread::spawn(move || {
                    handle_client(stream, my_gen, &access, &conn, &on_conn, &stop);
                    pending.fetch_sub(1, Ordering::SeqCst);
                });
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
    access: Arc<AccessStore>,
    stop: Arc<AtomicBool>,
    session_port: u16,
    my_name: String,
    device_id: String,
    decide: PairDecide,
    feedback: PairFeedback,
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

/// One pairing attempt. Returns true if the user accepted.
fn handle_pair(mut stream: TcpStream, ctx: &PairCtx) -> bool {
    let peer = stream.peer_addr().ok();
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(PAIR_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PAIR_IO_TIMEOUT));

    let decide = ctx.decide.clone();
    let mut asked = false;
    let t0 = Instant::now();
    match protocol::pairing::pair_responder_authorized_with_identity(
        &mut stream,
        ctx.session_port,
        &ctx.my_name,
        &ctx.device_id,
        |req| {
            asked = true;
            let revision = ctx.access.revision();
            if !decide(req) || ctx.stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let key = ctx
                .access
                .grant_if_revision(&req.peer_name, &req.device_id, revision)?;
            if ctx.stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            Ok(Some(key))
        },
    ) {
        Ok(PairDecision::Accepted(req)) => {
            if !ctx.stop.load(Ordering::SeqCst) {
                (ctx.feedback)(PairingNotice::Saved(req.peer_name.clone()));
            }
            plog!(
                "paired with {:?} ({peer:?}) after {:.1}s",
                req.peer_name,
                t0.elapsed().as_secs_f32()
            );
            true
        }
        Ok(PairDecision::Declined(req)) => {
            plog!(
                "declined {:?} ({peer:?}) after {:.1}s — a decline at ~{}s is the auto-decline, not a click",
                req.peer_name,
                t0.elapsed().as_secs_f32(),
                protocol::PAIR_DECISION_TIMEOUT.as_secs()
            );
            false
        }
        Err(e) => {
            if asked && !ctx.stop.load(Ordering::SeqCst) {
                (ctx.feedback)(PairingNotice::Failed(e.to_string()));
            }
            // Port scanners and half-open probes land here too, so this is not
            // worth surfacing in the UI — but it IS worth recording, because a
            // request that dies here never reaches `decide` and so never puts a
            // dialog on screen.
            plog!(
                "pairing attempt from {peer:?} failed after {:.1}s: {e}",
                t0.elapsed().as_secs_f32()
            );
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
                    plog!(
                        "request from {peer:?} REJECTED: another request already owns the dialog"
                    );
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
    access: Arc<AccessStore>,
    decide: PairDecide,
    feedback: PairFeedback,
) -> Option<(u16, Arc<AtomicBool>, JoinHandle<()>)> {
    let preferred = protocol::default_pairing_port(cfg.port);
    let listener = TcpListener::bind(("0.0.0.0", preferred))
        .or_else(|e| {
            eprintln!(
                "pairing port {preferred} unavailable ({e}); falling back to an ephemeral port"
            );
            TcpListener::bind(("0.0.0.0", 0))
        })
        .ok()?;
    let port = listener.local_addr().ok()?.port();
    if listener.set_nonblocking(true).is_err() {
        return None;
    }
    plog!(
        "pairing listener on 0.0.0.0:{port} (session port {})",
        cfg.port
    );

    let stop = Arc::new(AtomicBool::new(false));
    let ctx = Arc::new(PairCtx {
        access,
        stop: stop.clone(),
        session_port: cfg.port,
        my_name: this_device_name(),
        device_id: cfg.device_id.clone(),
        decide,
        feedback,
        busy: Arc::new(AtomicBool::new(false)),
        cooldown_until: Arc::new(Mutex::new(None)),
    });
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
fn advertise_mdns(
    port: u16,
    pair_port: Option<u16>,
    desktop_port: Option<u16>,
    device_id: &str,
) -> Option<(mdns_sd::ServiceDaemon, String)> {
    let host = this_device_name();
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("mDNS advertising unavailable: {e}");
            return None;
        }
    };

    // Advertise ONLY addresses this process can actually serve. `addr_auto` below
    // otherwise publishes every address of every monitored interface, and four of
    // the five records the Mac saw were dead ends:
    //
    //     [fe80::f9b9:…, ::1, ::5661:…, ::453e:…, 192.168.68.55]
    //
    //   * IPv6 of any kind. Both listeners bind `0.0.0.0`, which is IPv4-only, so
    //     NOTHING on this machine ever answers on an IPv6 address — these are not
    //     "sometimes routable", they are guaranteed to fail. `fe80::f9b9:…` is the
    //     one that cost ~75 s per connect attempt before #5 taught the sender to
    //     race the whole list; deleting it at the source is better than routing
    //     around it, and it spares the sender four wasted sockets every time.
    //   * loopback — enabled by default in mdns-sd. `::1` goes with the rest of
    //     IPv6; `127.0.0.1` never leaves this host (a browser on the LAN cannot
    //     see it, only one running here can), but publishing an address that means
    //     "me" to whoever reads it has no upside either.
    //
    // Ignore the errors: a daemon that will not narrow its interfaces still
    // advertises the right addresses among the wrong ones, which is strictly
    // better than not advertising at all.
    let _ = daemon.disable_interface(vec![
        mdns_sd::IfKind::LoopbackV4,
        mdns_sd::IfKind::LoopbackV6,
        mdns_sd::IfKind::IPv6,
    ]);
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

    props.insert(protocol::desktop::TXT_ID.into(), device_id.into());
    if let Some(p) = desktop_port {
        props.insert(protocol::desktop::TXT_PORT.into(), p.to_string());
        props.insert(
            protocol::desktop::TXT_VERSION.into(),
            protocol::desktop::VERSION.to_string(),
        );
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
    pub port: u16,
    desktop: Option<crate::desktop::Handle>,
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
        self.desktop.take();
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
#[cfg(not(windows))]
pub fn start<F: Fn(ConnStatus) + Send + Sync + 'static>(
    cfg: &protocol::config::Config,
    on_conn: F,
    decide: PairDecide,
) -> io::Result<Handle> {
    start_with_access(cfg, AccessStore::open(cfg)?, on_conn, decide)
}

pub fn start_with_access<F: Fn(ConnStatus) + Send + Sync + 'static>(
    cfg: &protocol::config::Config,
    access: Arc<AccessStore>,
    on_conn: F,
    decide: PairDecide,
) -> io::Result<Handle> {
    start_with_feedback(
        cfg,
        access,
        on_conn,
        decide,
        Arc::new(|notice| match notice {
            PairingNotice::Saved(name) => println!("paired controller: {name}"),
            PairingNotice::Failed(error) => eprintln!("pairing could not complete: {error}"),
        }),
    )
}

pub fn start_with_feedback<F: Fn(ConnStatus) + Send + Sync + 'static>(
    cfg: &protocol::config::Config,
    access: Arc<AccessStore>,
    on_conn: F,
    decide: PairDecide,
    feedback: PairFeedback,
) -> io::Result<Handle> {
    let listener = TcpListener::bind(("0.0.0.0", cfg.port))?; // port error -> GUI
    let mut resolved = cfg.clone();
    resolved.port = listener.local_addr()?.port();
    let cfg = &resolved;
    listener.set_nonblocking(true)?;
    println!(
        "keyboard-it host listening on 0.0.0.0:{} — waiting for connection",
        cfg.port
    );

    let stop = Arc::new(AtomicBool::new(false));
    let conn: ConnSlot = Arc::new(Mutex::new(None));
    let on_conn: OnConn = Arc::new(on_conn);
    let desktop = crate::desktop::start_with_access(cfg, access.clone(), on_conn.clone()).ok();
    let (s, c) = (stop.clone(), conn.clone());
    let auth = access.clone();
    let thread = thread::spawn(move || accept_loop(listener, auth, &s, &c, &on_conn));

    // Non-fatal: without it, already-paired Macs still connect.
    let pair = start_pair_listener(cfg, access, decide, feedback);
    let pair_port = pair.as_ref().map(|(p, _, _)| *p);

    Ok(Handle {
        port: cfg.port,
        stop,
        conn,
        thread: Some(thread),
        pair: pair.map(|(_, stop, thread)| (stop, thread)),
        mdns: advertise_mdns(
            cfg.port,
            pair_port,
            desktop.as_ref().map(|h| h.port),
            &cfg.device_id,
        ),
        desktop,
    })
}

/// Non-Windows dry-run: blocking variant (no stop; listens until the process dies).
/// This diagnostic binary has no consent UI, so it refuses new pairings.
/// Use keyboard-it-host --dry-run for interactive, code-verified pairing.
#[cfg(not(windows))]
pub fn serve(
    cfg: &protocol::config::Config,
    on_conn: impl Fn(ConnStatus) + Send + Sync + 'static,
) -> io::Result<()> {
    println!("(no injection on this platform — incoming keys are only printed [dry-run])");
    let decide: PairDecide = Arc::new(|req: &PairRequest| {
        println!(
            "[dry-run] pairing request from {:?}, fingerprint {} — declined; use keyboard-it-host --dry-run",
            req.peer_name,
            protocol::pairing::code_display(&req.code)
        );
        false
    });
    let handle = start(cfg, on_conn, decide)?;
    println!("receiver ready on port {}", handle.port);
    loop {
        thread::park();
    }
}

#[cfg(all(test, not(windows)))]
mod access_tests {
    use super::*;
    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        (client, listener.accept().unwrap().0)
    }
    fn context(
        access: Arc<AccessStore>,
        allow: bool,
        stopped: bool,
        feedback: PairFeedback,
    ) -> PairCtx {
        PairCtx {
            access,
            stop: Arc::new(AtomicBool::new(stopped)),
            session_port: 5599,
            my_name: "Simulated PC".into(),
            device_id: "pc-id".into(),
            decide: Arc::new(move |_| allow),
            feedback,
            busy: Arc::new(AtomicBool::new(false)),
            cooldown_until: Arc::new(Mutex::new(None)),
        }
    }
    #[test]
    fn replayed_handshake_cannot_displace_a_live_input_session() {
        use std::io::{Read, Write};
        // Record only a first flight, as an on-path observer could. The replay
        // client below never learns the PSK or the initiator's ephemeral secret.
        #[derive(Default)]
        struct FirstFlight(Vec<u8>);
        impl Read for FirstFlight {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> { Ok(0) }
        }
        impl Write for FirstFlight {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let _guard = crate::inject::TEST_CONTROLLER.lock().unwrap();
        let access = AccessStore::in_memory("replay-test");
        let psk = protocol::secure::psk_from_secret("replay-test");
        let mut recorded = FirstFlight::default();
        assert!(protocol::secure::handshake_initiator(&mut recorded, &psk).is_err());
        let conn: ConnSlot = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let on_conn: OnConn = Arc::new(|_| {});
        let (mut legitimate, server) = pair();
        let (a, c, o, s) = (access.clone(), conn.clone(), on_conn.clone(), stop.clone());
        let live = thread::spawn(move || handle_client(server, 1, &a, &c, &o, &s));
        let mut transport = protocol::secure::handshake_initiator(&mut legitimate, &psk).unwrap();
        protocol::secure::send_event(&mut transport, &mut legitimate,
            &protocol::InputEvent::MouseMove { dx: 0, dy: 0 }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while conn.lock().unwrap().as_ref().map(|v| v.0) != Some(1) {
            assert!(Instant::now() < deadline, "legitimate session did not attach");
            thread::sleep(Duration::from_millis(5));
        }
        let (mut attacker, replay_socket) = pair();
        let (a, c, o, s) = (access, conn.clone(), on_conn, stop);
        let replay = thread::spawn(move || handle_client(replay_socket, 2, &a, &c, &o, &s));
        attacker.write_all(&recorded.0).unwrap();
        let mut length = [0; 4];
        attacker.read_exact(&mut length).unwrap();
        let mut response = vec![0; u32::from_be_bytes(length) as usize];
        attacker.read_exact(&mut response).unwrap();
        // No fresh transport proof can be produced from the captured flight.
        drop(attacker);
        replay.join().unwrap();
        let retained = conn.lock().unwrap().as_ref().map(|v| v.0) == Some(1);
        drop(legitimate);
        live.join().unwrap();
        assert!(retained, "replaying a captured handshake displaced the live controller");
    }

    #[test]
    fn fresh_receiver_pairs_without_any_legacy_shared_key() {
        let access = AccessStore::in_memory("");
        let (mut client, server) = pair();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = context(
            access.clone(),
            true,
            false,
            Arc::new(move |notice| {
                tx.send(notice).unwrap();
            }),
        );
        let worker = thread::spawn(move || handle_pair(server, &ctx));
        let outcome =
            protocol::pairing::pair_initiator_with_identity(&mut client, "Mac", "mac-id", |_| {})
                .unwrap();
        assert_eq!(outcome.peer_device_id, "pc-id");
        assert!(worker.join().unwrap());
        assert!(matches!(rx.recv().unwrap(), PairingNotice::Saved(name) if name == "Mac"));
        assert_eq!(
            access.authentication_keys()[0].key,
            protocol::secure::psk_from_secret(&outcome.secret)
        );
    }
    #[test]
    fn declined_or_stopped_pairing_never_grants_access() {
        for (allow, stopped) in [(false, false), (true, true)] {
            let access = AccessStore::in_memory("");
            let (mut client, server) = pair();
            let ctx = context(
                access.clone(),
                allow,
                stopped,
                Arc::new(|_| panic!("unexpected completion notice")),
            );
            let worker = thread::spawn(move || handle_pair(server, &ctx));
            assert!(protocol::pairing::pair_initiator_with_identity(
                &mut client,
                "Mac",
                "id",
                |_| {}
            )
            .is_err());
            assert!(!worker.join().unwrap());
            assert!(access.controllers().is_empty());
        }
    }
    #[test]
    fn authorization_failure_reports_failure_instead_of_approval() {
        let access = AccessStore::in_memory("");
        for i in 0..protocol::access::MAX_CONTROLLERS {
            access.grant("PC", &format!("id-{i}")).unwrap();
        }
        let (mut client, server) = pair();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = context(
            access,
            true,
            false,
            Arc::new(move |notice| {
                tx.send(notice).unwrap();
            }),
        );
        let worker = thread::spawn(move || handle_pair(server, &ctx));
        assert!(protocol::pairing::pair_initiator_with_identity(
            &mut client,
            "Extra",
            "extra",
            |_| {}
        )
        .is_err());
        assert!(!worker.join().unwrap());
        assert!(
            matches!(rx.recv().unwrap(), PairingNotice::Failed(message) if message.contains("limit"))
        );
    }
    #[test]
    fn legacy_input_socket_closes_and_releases_ownership_on_individual_revocation() {
        use std::io::Read;
        let _guard = crate::inject::TEST_CONTROLLER.lock().unwrap();
        let access = AccessStore::in_memory("");
        let secret = access.grant("Mac", "mac-id").unwrap();
        let key = access.authentication_keys()[0].clone();
        let (mut client, server) = pair();
        let conn = Arc::new(Mutex::new(None));
        let stop = AtomicBool::new(false);
        let (tx, rx) = std::sync::mpsc::channel();
        let on_conn: OnConn = Arc::new(move |status| {
            tx.send(status).unwrap();
        });
        let server_access = access.clone();
        let worker =
            thread::spawn(move || handle_client(server, 1, &server_access, &conn, &on_conn, &stop));
        let mut transport = protocol::secure::handshake_initiator(
            &mut client,
            &protocol::secure::psk_from_secret(&secret),
        )
        .unwrap();
        protocol::secure::send_event(
            &mut transport,
            &mut client,
            &protocol::InputEvent::Key(protocol::KeyEvent {
                msg: protocol::MsgType::Down,
                hid_usage: 4,
                modifiers: 0,
            }),
        )
        .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), ConnStatus::Connected);
        access.revoke(&key.id).unwrap();
        let closed = match client.read(&mut [0]) {
            Ok(0) => true,
            Err(e) => matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ),
            _ => false,
        };
        worker.join().unwrap();
        assert!(closed, "revocation must close the input socket");
        assert!(!inject::connected());
    }
}
