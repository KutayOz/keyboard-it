//! Native settings window — the zero-typing replacement for editing config.toml.
//!
//! A "Your Windows PC" popup fed by mDNS discovery, "Pair & Connect", Start at
//! Login, and a live status line. Nothing here is typed: pairing fetches the key
//! from the PC once the user approves the request there, writes the config, and
//! flips capture::CONFIG_DIRTY so the background connection picks it up within
//! about a second — no restart.
//!
//! Threading: AppKit objects are main-thread-only. Two background threads write
//! shared state and never touch AppKit — the mDNS browser (crate::discovery, a
//! process-wide singleton shared with the permissions window) and the pairing
//! worker (an Arc<Mutex<PairState>>) — and a 1 s main-thread NSTimer mirrors
//! both into the window (same pattern as menubar::install_status_updater).

#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSPopUpButton, NSTextField, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSTimer,
};

use crate::discovery::{self, DiscoveredPeer};
use crate::menubar::ConnStatus;

/// What the pairing worker is doing, rendered by the 1 s timer. Kept as
/// pre-rendered text because only the worker knows the peer name and the code,
/// and the timer must not have to re-derive them.
#[derive(Clone, PartialEq, Eq)]
enum PairState {
    /// Nothing in flight — the status line falls back to the connection state.
    Idle,
    /// In progress; the button stays disabled.
    Busy(String),
    /// The request is on the PC's screen and a human has to answer it. Held as a
    /// deadline rather than a rendered string because this is the one state where
    /// the user has to physically go somewhere, and "how long have I got" is the
    /// only question they have. The 1 s tick re-renders the remaining time.
    AwaitingApproval { peer: String, deadline: Instant },
    /// Terminal message, shown until `until` and then released back to Idle.
    Finished { text: String, until: Instant },
}

/// Read timeout while waiting for the user to click Allow on the PC. Deliberately
/// longer than the receiver's own auto-decline, so running out of time arrives as
/// an explicit decline rather than as a dead socket this side cannot explain.
const PAIR_DECISION_WAIT: Duration = protocol::PAIR_DECISION_WAIT;

/// Timeout for everything before the human: TCP connect, handshake, name exchange.
const PAIR_IO_TIMEOUT: Duration = Duration::from_secs(10);

// The controller is main-thread-only (holds AppKit objects), so a thread_local
// is the natural owner — no Send/Sync juggling for a single-window app.
thread_local! {
    static CONTROLLER: RefCell<Option<Retained<SettingsController>>> = const { RefCell::new(None) };
}

/// Connection state source, registered by capture::run. The window's status line
/// mirrors the same AtomicU8 the menu bar title uses, without owning the thread.
static CONN_STATUS: OnceLock<Arc<AtomicU8>> = OnceLock::new();

pub fn set_conn_status_source(src: Arc<AtomicU8>) {
    let _ = CONN_STATUS.set(src);
}

/// Is there a live session with the PC right now? Read by the permissions window:
/// an established TCP connection to a machine on this LAN is proof that Local
/// Network access works, and it is the only such proof that does not depend on
/// mDNS finding anything.
pub fn is_connected() -> bool {
    CONN_STATUS
        .get()
        .is_some_and(|s| ConnStatus::from_u8(s.load(Ordering::Relaxed)) == ConnStatus::Connected)
}

fn conn_status_text(s: ConnStatus) -> &'static str {
    match s {
        ConnStatus::ConfigNeeded => "Not paired — pick your PC below and click Pair & Connect.",
        ConnStatus::Connecting => "Connecting…",
        ConnStatus::Connected => "Connected.",
        ConnStatus::Disconnected => "No connection — retrying in the background.",
        ConnStatus::HandshakeFailed => "The PC rejected the key — pair again.",
        ConnStatus::Unreachable => "Can't reach your PC — pick it again and Pair & Connect.",
    }
}

/// Open (create on first use) the settings window and bring it to the front.
pub fn open(mtm: MainThreadMarker) {
    CONTROLLER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(SettingsController::create(mtm));
        }
        let c = slot.as_ref().unwrap();
        // Reload from disk only when hidden: reopening must not clobber edits in
        // progress when the window is already on screen.
        if !c.ivars().window.isVisible() {
            c.load_fields_from_config();
        }
        // Accessory app (no Dock icon): without an explicit activation the window
        // can appear behind whatever the user was working in.
        let app = NSApplication::sharedApplication(mtm);
        unsafe {
            let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
        }
        c.ivars().window.makeKeyAndOrderFront(None);
    });
}

/// Ivars of the controller: the window, its controls, and the discovery state.
struct Ivars {
    window: Retained<NSWindow>,
    popup: Retained<NSPopUpButton>,
    autostart_check: Retained<NSButton>,
    status_label: Retained<NSTextField>,
    /// The line under the popup. Normally tells the user what to do next; turns
    /// into a diagnosis once discovery has come up empty for long enough.
    hint_label: Retained<NSTextField>,
    paired_label: Retained<NSTextField>,
    pair_button: Retained<NSButton>,
    unpair_button: Retained<NSButton>,
    /// The list currently rendered in the popup; popupSelected: indexes into
    /// THIS (not `peers`) so a refresh between click and action cannot drift.
    shown_peers: RefCell<Vec<DiscoveredPeer>>,
    /// Written by the pairing worker thread, read by the 1 s timer.
    pair_state: Arc<Mutex<PairState>>,
    /// While set and in the future, the timer must not overwrite the status line
    /// (transient feedback would otherwise vanish within a second).
    status_hold: Cell<Option<Instant>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KbItSettingsController"]
    #[ivars = Ivars]
    struct SettingsController;

    unsafe impl NSObjectProtocol for SettingsController {}

    impl SettingsController {
        // Selecting a PC only moves the highlight; pairing is an explicit click,
        // because it puts a dialog on someone else's screen.
        #[unsafe(method(popupSelected:))]
        fn popupSelected(&self, _sender: Option<&AnyObject>) {
            self.refresh_buttons();
        }

        // Checkbox applies immediately (same behavior as the menu bar item); the
        // timer re-syncs it from disk, so a failed toggle snaps back visibly.
        #[unsafe(method(toggleAutostart:))]
        fn toggleAutostart(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            let state: isize = unsafe { msg_send![&*iv.autostart_check, state] };
            let _ = crate::autostart::set_enabled(state == 1);
            self.sync_autostart();
        }

        // "Pair & Connect" -> hand the selected PC to a worker thread. Everything
        // after this point is network + a human on the other machine, so it must
        // not run on the main thread.
        #[unsafe(method(pairAndConnect:))]
        fn pairAndConnect(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            let Some(peer) = self.selected_peer() else {
                self.set_status("Choose your PC from the list first.", Duration::from_secs(5));
                return;
            };
            // Guard against a double click landing two workers on one answer slot.
            if !matches!(*iv.pair_state.lock().unwrap(), PairState::Idle) {
                return;
            }
            *iv.pair_state.lock().unwrap() = PairState::Busy(format!("Contacting {}…", peer.name));
            self.refresh_buttons();
            spawn_pairing(peer, iv.pair_state.clone());
        }

        // "Unpair" -> forget the PC locally. The PC keeps its key; pairing again
        // needs another approval there.
        #[unsafe(method(unpair:))]
        fn unpair(&self, _sender: Option<&AnyObject>) {
            let mut cfg = load_config();
            cfg.shared_secret.clear();
            cfg.peer_host.clear();
            cfg.peer_ip.clear();
            match cfg.save() {
                Ok(()) => {
                    crate::capture::CONFIG_DIRTY.store(true, Ordering::Relaxed);
                    self.set_status("Unpaired.", Duration::from_secs(4));
                }
                Err(e) => self.set_status(&format!("Could not unpair: {e}"), Duration::from_secs(6)),
            }
            self.refresh_paired_label();
            self.refresh_buttons();
        }

        // 1 s heartbeat while the window is visible: discovery list -> popup,
        // autostart state -> checkbox, pairing/connection state -> status line.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            if !self.ivars().window.isVisible() {
                return;
            }
            self.refresh_popup();
            self.refresh_hint();
            self.sync_autostart();
            self.refresh_status();
            self.refresh_buttons();
        }
    }
);

impl SettingsController {
    /// Build the window + controls, wire targets, start discovery and the timer.
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
        let content = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(460.0, 210.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                content,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Programmatic NSWindows default to releasedWhenClosed=YES, which would
        // over-release under Retained on close — objc2 requires turning it off.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("keyboard-it Settings"));
        window.center();

        let label = |text: &NSString, x: f64, y: f64, w: f64| -> Retained<NSTextField> {
            let l = NSTextField::labelWithString(text, mtm);
            l.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, 17.0)));
            l
        };

        let pc_label = label(ns_string!("Your Windows PC"), 16.0, 168.0, 118.0);
        let popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(138.0, 162.0), NSSize::new(306.0, 26.0)),
            false,
        );
        // Placeholder until discovery reports something (refresh_popup rebuilds).
        popup.addItemWithTitle(ns_string!("Searching your network…"));
        popup.setEnabled(false);

        let hint = label(
            ns_string!("Pick your PC, then click Allow on it."),
            138.0,
            136.0,
            306.0,
        );

        let pair_button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("Pair & Connect"), None, None, mtm)
        };
        pair_button.setFrame(NSRect::new(NSPoint::new(294.0, 96.0), NSSize::new(150.0, 32.0)));
        // Return triggers pairing — the whole flow works keyboard-only.
        pair_button.setKeyEquivalent(ns_string!("\r"));

        let paired_label = label(ns_string!(""), 16.0, 103.0, 270.0);
        let unpair_button =
            unsafe { NSButton::buttonWithTitle_target_action(ns_string!("Unpair"), None, None, mtm) };
        unpair_button.setFrame(NSRect::new(NSPoint::new(16.0, 56.0), NSSize::new(100.0, 32.0)));

        let autostart_check = unsafe {
            NSButton::checkboxWithTitle_target_action(ns_string!("Start at Login"), None, None, mtm)
        };
        autostart_check.setFrame(NSRect::new(NSPoint::new(130.0, 64.0), NSSize::new(220.0, 18.0)));

        let status_label = label(ns_string!(""), 16.0, 20.0, 428.0);

        let content_view = window.contentView().expect("titled window has a content view");
        for view in [&*pc_label, &*hint, &*paired_label, &*status_label] {
            content_view.addSubview(view);
        }
        content_view.addSubview(&popup);
        content_view.addSubview(&pair_button);
        content_view.addSubview(&unpair_button);
        content_view.addSubview(&autostart_check);

        // Idempotent: the permissions window may already have started the browse
        // (and with it raised the Local Network prompt) — this is then a no-op.
        discovery::start();

        let this = Self::alloc(mtm).set_ivars(Ivars {
            window,
            popup: popup.clone(),
            autostart_check: autostart_check.clone(),
            status_label,
            hint_label: hint.clone(),
            paired_label,
            pair_button: pair_button.clone(),
            unpair_button: unpair_button.clone(),
            shown_peers: RefCell::new(Vec::new()),
            pair_state: Arc::new(Mutex::new(PairState::Idle)),
            status_hold: Cell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Targets are wired after init: the controls must exist before the
        // controller (they live in its ivars), so they start target-less.
        let wire = |control: &NSButton, action: Sel| unsafe {
            control.setTarget(Some(&*this));
            control.setAction(Some(action));
        };
        wire(&autostart_check, sel!(toggleAutostart:));
        wire(&pair_button, sel!(pairAndConnect:));
        wire(&unpair_button, sel!(unpair:));
        unsafe {
            popup.setTarget(Some(&*this));
            popup.setAction(Some(sel!(popupSelected:)));
        }

        // Main-thread timer: the browser thread cannot touch AppKit, so the popup
        // (and status/checkbox) are refreshed from here once per second.
        unsafe {
            let _ = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                1.0,
                &this,
                sel!(tick:),
                None,
                true,
            );
        }
        this
    }

    /// Re-read config-derived UI (called when the window opens).
    fn load_fields_from_config(&self) {
        self.refresh_paired_label();
        self.refresh_hint();
        self.sync_autostart();
        self.refresh_status();
        self.refresh_buttons();
    }

    /// "Paired with DESKTOP-ABC" / "Not paired yet".
    fn refresh_paired_label(&self) {
        let cfg = load_config();
        let text = if cfg.is_complete() {
            format!("Paired with {}", cfg.peer_host)
        } else {
            "Not paired yet".to_string()
        };
        let iv = self.ivars();
        if iv.paired_label.stringValue().to_string() != text {
            iv.paired_label.setStringValue(&NSString::from_str(&text));
        }
    }

    /// The peer highlighted in the popup, if any. Reads `shown_peers` (not
    /// `peers`) so a refresh between click and action cannot shift the index.
    fn selected_peer(&self) -> Option<DiscoveredPeer> {
        let iv = self.ivars();
        let idx = iv.popup.indexOfSelectedItem();
        if idx < 1 {
            return None; // index 0 is the placeholder row
        }
        iv.shown_peers.borrow().get((idx - 1) as usize).cloned()
    }

    /// Enable/disable the two buttons from live state. Cheap enough to run on
    /// every tick, which keeps them correct without any change notifications.
    fn refresh_buttons(&self) {
        let iv = self.ivars();
        let pairing = !matches!(*iv.pair_state.lock().unwrap(), PairState::Idle);
        iv.pair_button.setEnabled(!pairing && self.selected_peer().is_some());
        iv.unpair_button.setEnabled(!pairing && load_config().is_complete());
    }

    /// True once the browse has run long enough with nothing to show that
    /// "Searching your network…" has stopped being an honest answer.
    fn discovery_stalled(&self) -> bool {
        discovery::stalled()
    }

    /// The line under the popup: what to do next, or why there is nothing to do.
    fn refresh_hint(&self) {
        let iv = self.ivars();
        let text = if self.discovery_stalled() {
            "Nothing is answering on this network."
        } else {
            "Pick your PC, then click Allow on it."
        };
        if iv.hint_label.stringValue().to_string() != text {
            iv.hint_label.setStringValue(&NSString::from_str(text));
        }
    }

    /// Show a message and keep the timer from overwriting it for `hold`.
    fn set_status(&self, text: &str, hold: Duration) {
        let iv = self.ivars();
        iv.status_label.setStringValue(&NSString::from_str(text));
        iv.status_hold.set(Some(Instant::now() + hold));
    }

    /// Mirror the LaunchAgent state (also toggled via the menu bar item).
    fn sync_autostart(&self) {
        let iv = self.ivars();
        let disk: isize = if crate::autostart::is_enabled() { 1 } else { 0 };
        let shown: isize = unsafe { msg_send![&*iv.autostart_check, state] };
        if shown != disk {
            unsafe {
                let _: () = msg_send![&*iv.autostart_check, setState: disk];
            }
        }
    }

    /// Drive the status line. Pairing progress outranks the connection state:
    /// while a request is in flight, "Confirm on DESKTOP-ABC — code 482 913" is
    /// the only thing the user needs, and the background reconnect loop would
    /// otherwise keep overwriting it with "No connection".
    fn refresh_status(&self) {
        let iv = self.ivars();

        let pair_text = {
            let mut state = iv.pair_state.lock().unwrap();
            match &*state {
                PairState::Idle => None,
                PairState::Busy(text) => Some(text.clone()),
                // The instruction comes first and the clock second: the user is
                // about to walk away from this screen, so what they carry with
                // them has to be what to do, not how long they have.
                PairState::AwaitingApproval { peer, deadline } => {
                    let left = deadline.saturating_duration_since(Instant::now()).as_secs();
                    Some(format!("Go to {peer} and click Allow — {left}s left"))
                }
                PairState::Finished { text, until } => {
                    if Instant::now() < *until {
                        Some(text.clone())
                    } else {
                        // Released back to the connection status, and the config
                        // written by the worker is now reflected in the labels.
                        *state = PairState::Idle;
                        self.refresh_paired_label();
                        None
                    }
                }
            }
        };
        if let Some(text) = pair_text {
            if iv.status_label.stringValue().to_string() != text {
                iv.status_label.setStringValue(&NSString::from_str(&text));
            }
            return;
        }

        if let Some(until) = iv.status_hold.get() {
            if Instant::now() < until {
                return;
            }
            iv.status_hold.set(None);
        }
        let Some(src) = CONN_STATUS.get() else { return };
        let conn = ConnStatus::from_u8(src.load(Ordering::Relaxed));
        // A stalled browse outranks the connection line. "No connection —
        // retrying in the background" is true, but it describes a link that
        // cannot be established while the real problem is that the user has no
        // way to pick a PC in the first place. Say the thing they can act on.
        let text = if conn != ConnStatus::Connected && self.discovery_stalled() {
            "No PCs found — check Privacy & Security \u{203A} Local Network."
        } else {
            conn_status_text(conn)
        };
        if iv.status_label.stringValue().to_string() != text {
            iv.status_label.setStringValue(&NSString::from_str(text));
        }
    }

    /// Rebuild the popup when the discovered list changed since the last render.
    fn refresh_popup(&self) {
        let iv = self.ivars();
        let mut now: Vec<DiscoveredPeer> = discovery::peers();
        // Stable order: the browser thread appends in resolve order, which would
        // make entries jump around between refreshes.
        now.sort_by(|a, b| a.name.cmp(&b.name));
        let popup = &iv.popup;

        // The empty case cannot sit behind the list-changed guard below: the list
        // stays empty either way, but the REASON it is empty flips from "still
        // looking" to "nothing is coming" when the grace period runs out, and that
        // is the only signal the user gets.
        if now.is_empty() {
            let want = if self.discovery_stalled() {
                "No PCs found"
            } else {
                "Searching your network…"
            };
            // Compare against item 0 rather than the selected item: whether a
            // freshly repopulated popup auto-selects anything is not something to
            // depend on, and if it did not, this would rebuild on every tick.
            let have = (popup.numberOfItems() == 1)
                .then(|| popup.itemTitleAtIndex(0).to_string());
            if have.as_deref() != Some(want) {
                popup.removeAllItems();
                popup.addItemWithTitle(&NSString::from_str(want));
                popup.setEnabled(false);
            }
            iv.shown_peers.borrow_mut().clear();
            return;
        }

        if now == *iv.shown_peers.borrow() {
            return;
        }
        let selected = popup.titleOfSelectedItem();
        popup.removeAllItems();
        popup.setEnabled(true);
        popup.addItemWithTitle(ns_string!("Choose a discovered PC…"));
        for p in &now {
            // Flag the ones that cannot be paired with, rather than letting
            // the user pick them and fail.
            let title = if p.pair_port.is_some() {
                format!("{} ({})", p.name, p.host)
            } else {
                format!("{} ({}) — needs a newer keyboard-it", p.name, p.host)
            };
            popup.addItemWithTitle(&NSString::from_str(&title));
        }
        // Keep the user's selection across rebuilds when it still exists.
        if let Some(title) = selected {
            let idx = popup.indexOfItemWithTitle(&title);
            if idx >= 0 {
                popup.selectItemAtIndex(idx);
            }
        }
        *iv.shown_peers.borrow_mut() = now;
    }
}

// The Edit menu that used to live here existed solely so Cmd+V worked in the
// pairing-key field. There are no text fields left, so it went with them.

fn load_config() -> protocol::config::Config {
    protocol::config::Config::load().ok().flatten().unwrap_or_default()
}

/// This Mac's name as the user knows it ("Kutay's MacBook Pro"), shown in the
/// confirmation dialog on the PC. `scutil` is the only place the friendly name
/// lives; the POSIX hostname is a mangled version of it.
fn this_mac_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".to_string())
}

/// Run one pairing attempt off the main thread and report progress through
/// `state`. Every exit path writes a `Finished` message: the button stays
/// disabled until the state returns to Idle, so a silent return would wedge it.
fn spawn_pairing(peer: DiscoveredPeer, state: Arc<Mutex<PairState>>) {
    std::thread::spawn(move || {
        let set = |s: PairState| {
            if let Ok(mut g) = state.lock() {
                *g = s;
            }
        };
        let done = |text: String, secs: u64| {
            set(PairState::Finished {
                text,
                until: Instant::now() + Duration::from_secs(secs),
            })
        };

        let Some(pair_port) = peer.pair_port else {
            done(
                format!("{} is running an older keyboard-it — update it on the PC.", peer.name),
                8,
            );
            return;
        };

        let addr = format!("{}:{}", peer.host, pair_port);
        // The address came from mDNS as a literal IP, so this parses and gets a
        // real connect timeout; the by-name fallback is belt and braces.
        let connected = match addr.parse::<SocketAddr>() {
            Ok(sa) => TcpStream::connect_timeout(&sa, PAIR_IO_TIMEOUT),
            Err(_) => TcpStream::connect(&addr),
        };
        let mut stream = match connected {
            Ok(s) => s,
            Err(e) => {
                done(format!("Could not reach {}: {e}", peer.name), 8);
                return;
            }
        };
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(PAIR_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(PAIR_IO_TIMEOUT));
        // A dup of the same socket: setting a timeout through it affects the one
        // underlying socket, which is how the read timeout gets extended at the
        // exact moment the wait turns into "waiting for a human".
        let ctl = stream.try_clone().ok();

        let name = this_mac_name();
        // When the request actually landed on the PC's screen. The wire cannot
        // tell an explicit Deny apart from the receiver's auto-decline, but the
        // clock can: past the deadline, "declined" is the wrong word for it.
        let asked_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let asked_at_cb = asked_at.clone();
        // The callback is not about the code (which is no longer shown anywhere —
        // nobody compared it, so it was decoration). It fires at the one instant
        // that matters: the handshake is done and the wait has turned into waiting
        // for a human, which is when the read timeout has to grow — and when the
        // receiver's own decision clock starts, so the countdown starts here too.
        let result = protocol::pairing::pair_initiator(&mut stream, &name, |_code| {
            if let Some(c) = &ctl {
                let _ = c.set_read_timeout(Some(PAIR_DECISION_WAIT));
            }
            if let Ok(mut g) = asked_at_cb.lock() {
                *g = Some(Instant::now());
            }
            set(PairState::AwaitingApproval {
                peer: peer.name.clone(),
                deadline: Instant::now() + protocol::PAIR_DECISION_TIMEOUT,
            });
        });

        let outcome = match result {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // Same error either way, so distinguish them by the clock rather
                // than telling someone who simply ran out of time that their PC
                // turned them down.
                let expired = asked_at
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .is_some_and(|t| t.elapsed() >= protocol::PAIR_DECISION_TIMEOUT);
                if expired {
                    done(format!("No answer on {} — the request expired.", peer.name), 10);
                } else {
                    done(format!("{} declined the pairing.", peer.name), 8);
                }
                return;
            }
            Err(e) => {
                done(format!("Pairing failed: {e}"), 10);
                return;
            }
        };

        // Load-then-mutate: never rebuild the struct, or fields this window does
        // not know about are silently dropped.
        let mut cfg = load_config();
        cfg.role = protocol::config::Role::Sender;
        cfg.shared_secret = outcome.secret;
        cfg.port = outcome.session_port;
        // Prefer the mDNS name — it follows the PC to a new IP.
        cfg.peer_host =
            if peer.hostname.is_empty() { peer.host.clone() } else { peer.hostname.clone() };
        cfg.peer_ip = peer.host.clone();
        if let Err(e) = cfg.save() {
            done(format!("Paired, but the settings could not be saved: {e}"), 10);
            return;
        }
        // The connection thread polls this (~1 s) and reconnects with the new
        // key and address — no restart.
        crate::capture::CONFIG_DIRTY.store(true, Ordering::Relaxed);
        done(format!("Paired with {} — connecting…", outcome.peer_name), 5);
    });
}

