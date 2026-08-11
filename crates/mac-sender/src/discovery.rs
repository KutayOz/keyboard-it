//! Process-wide mDNS discovery of win-receiver instances.
//!
//! Lifted out of settings.rs so the permissions window and the settings window
//! share ONE ServiceDaemon. Two windows each opening their own browse would put
//! two multicast sockets on the wire for the same question — and on macOS 15+
//! the first multicast send is what raises the Local Network prompt, so "who
//! starts the browse" stopped being an implementation detail: it decides WHEN
//! the user is asked. `start()` is therefore explicit and idempotent, and the
//! permissions window calls it when Local Network is the step it is on.
//!
//! Threading: the browse runs on its own thread and only ever touches the
//! Mutex/atomics below. AppKit stays on the main thread, which polls via a timer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One receiver found via mDNS (win-receiver advertises protocol::MDNS_SERVICE).
/// `fullname` is the dedupe/removal key (ServiceRemoved only carries the fullname).
#[derive(Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub fullname: String,
    pub name: String,
    /// Resolved IP. Used to reach the PC now, and stored as the fallback address.
    pub host: String,
    /// The advertised ".local" name (trailing dot stripped). Preferred for the
    /// stored config: macOS resolves it via mDNS, so the link survives the PC
    /// getting a new DHCP lease.
    pub hostname: String,
    /// Session port (the SRV port).
    pub port: u16,
    /// Pairing port from the TXT record. `None` means the PC is running a build
    /// from before pairing existed.
    pub pair_port: Option<u16>,
}

/// How long the browse may return nothing before the UI stops saying
/// "Searching your network…" and starts saying why. Long enough that a healthy
/// network has answered several times over; short enough to beat the user's
/// patience. On macOS 15+ the overwhelmingly likely cause of a permanently
/// empty list is Local Network access, which has no API to query — the timeout
/// IS the detection.
pub const DISCOVERY_GRACE: Duration = Duration::from_secs(8);

struct Shared {
    peers: Mutex<Vec<DiscoveredPeer>>,
    started: Instant,
    /// Sticky, unlike `peers`: set the first time ANY service resolves and never
    /// cleared. A peer that later goes away (the PC was powered off) empties the
    /// list, and without this the Local Network permission row would flip back
    /// from "granted" to "denied" over a permission that never changed.
    ever_answered: AtomicBool,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// Start browsing. Idempotent — the second and later calls are no-ops, so both
/// windows can call it without coordinating. NOTE: the FIRST call is the one
/// that makes macOS put up the Local Network prompt.
pub fn start() {
    let mut spawned = None;
    SHARED.get_or_init(|| {
        let shared = Arc::new(Shared {
            peers: Mutex::new(Vec::new()),
            started: Instant::now(),
            ever_answered: AtomicBool::new(false),
        });
        // Spawn AFTER get_or_init returns, not inside it: a panic in the closure
        // would leave the OnceLock unset with a thread already running.
        spawned = Some(shared.clone());
        shared
    });
    if let Some(shared) = spawned {
        spawn_browser(shared);
    }
}

pub fn peers() -> Vec<DiscoveredPeer> {
    SHARED
        .get()
        .and_then(|s| s.peers.lock().ok().map(|g| g.clone()))
        .unwrap_or_default()
}

/// True once something has answered — i.e. Local Network access demonstrably
/// works. Never goes back to false.
pub fn ever_answered() -> bool {
    SHARED.get().is_some_and(|s| s.ever_answered.load(Ordering::Relaxed))
}

/// Time since the browse started; None when it has not been started.
pub fn elapsed() -> Option<Duration> {
    SHARED.get().map(|s| s.started.elapsed())
}

/// True once the browse has run long enough with nothing to show that
/// "Searching your network…" has stopped being an honest answer. Live-empty, so
/// a PC that goes away brings this back — that is what the settings window's
/// hint line wants (`ever_answered` is the permission-shaped question).
pub fn stalled() -> bool {
    SHARED.get().is_some_and(|s| {
        s.started.elapsed() >= DISCOVERY_GRACE
            && s.peers.lock().map(|p| p.is_empty()).unwrap_or(false)
    })
}

/// Browse protocol::MDNS_SERVICE for the lifetime of the process.
/// Only the shared state is touched from here — AppKit stays on the main thread.
fn spawn_browser(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        use mdns_sd::{ServiceDaemon, ServiceEvent};
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("mDNS discovery unavailable: {e}");
                return;
            }
        };
        let rx = match daemon.browse(protocol::MDNS_SERVICE) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("mDNS browse failed: {e}");
                return;
            }
        };
        while let Ok(event) = rx.recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let addrs: Vec<std::net::IpAddr> =
                        info.get_addresses().iter().map(|a| a.to_ip_addr()).collect();
                    // Rank, do not `min()`. Four of the five records a receiver
                    // advertises are typically unreachable, and the old "filter to
                    // IPv4 first" only hid that: one receiver with no A record and
                    // the fallback picks `::1`. See protocol::addr_rank.
                    let Some(ip) = protocol::pick_service_addr(addrs.iter()) else { continue };
                    let fullname = info.get_fullname().to_string();
                    let name = fullname
                        .strip_suffix(protocol::MDNS_SERVICE)
                        .map(|s| s.trim_end_matches('.').to_string())
                        .unwrap_or_else(|| fullname.clone());
                    // The advertised ".local." name outlives any particular DHCP
                    // lease, so it is what gets stored once pairing succeeds.
                    let hostname = info.get_hostname().trim_end_matches('.').to_string();
                    // Absent on receivers built before pairing existed.
                    let pair_port = info
                        .get_property_val_str(protocol::MDNS_TXT_PAIR_PORT)
                        .and_then(|s| s.parse::<u16>().ok())
                        .filter(|p| *p != 0);
                    let peer = DiscoveredPeer {
                        fullname: fullname.clone(),
                        name,
                        host: ip.to_string(),
                        hostname,
                        port: info.get_port(),
                        pair_port,
                    };
                    // Set before the list write: this is the "the network answered"
                    // signal, and it must hold even for a record we then drop.
                    shared.ever_answered.store(true, Ordering::Relaxed);
                    if let Ok(mut list) = shared.peers.lock() {
                        match list.iter_mut().find(|p| p.fullname == fullname) {
                            Some(existing) => *existing = peer,
                            None => list.push(peer),
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_ty, fullname) => {
                    if let Ok(mut list) = shared.peers.lock() {
                        list.retain(|p| p.fullname != fullname);
                    }
                }
                _ => {}
            }
        }
    });
}
