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
    NSApplication, NSBackingStoreType, NSButton, NSFont, NSPopUpButton, NSTextField, NSWindow,
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
    AwaitingApproval {
        peer: String,
        code: String,
        deadline: Instant,
    },
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
    if load_config().android_selected() { return false; }
    CONN_STATUS
        .get()
        .is_some_and(|s| ConnStatus::from_u8(s.load(Ordering::Relaxed)) == ConnStatus::Connected)
}

fn conn_status_text(s: ConnStatus) -> &'static str {
    match s {
        ConnStatus::ConfigNeeded => "Not paired — pick your device below and click Pair & Connect.",
        ConnStatus::Connecting => "Connecting…",
        ConnStatus::Connected => "Connected.",
        ConnStatus::Disconnected => "No connection — retrying in the background.",
        ConnStatus::HandshakeFailed => "The device rejected the key — pair again.",
        ConnStatus::Unreachable => "Can't reach your device — pick it again and Pair & Connect.",
    }
}

/// Open (create on first use) the settings window and bring it to the front.
pub fn open(mtm: MainThreadMarker) {
    CONTROLLER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(SettingsController::create(mtm, true));
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
    saved_popup: Retained<NSPopUpButton>,
    shown_devices: RefCell<Vec<(String, String)>>,
    connect_button: Retained<NSButton>,
    open_button: Retained<NSButton>,
    screen_check: Retained<NSButton>,
    input_check: Retained<NSButton>,
    clipboard_check: Retained<NSButton>,
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
        #[unsafe(method(addAndroid:))]
        fn add_android(&self, _sender: Option<&AnyObject>) { crate::android::open_setup(self.mtm()); }

        #[unsafe(method(savedSelected:))]
        fn savedSelected(&self, _sender: Option<&AnyObject>) {
            self.refresh_permissions();
            self.refresh_buttons();
            self.refresh_hint();
        }

        #[unsafe(method(connectSaved:))]
        fn connectSaved(&self, _sender: Option<&AnyObject>) {
            let Some(id) = self.selected_device() else { return; };
            crate::desktop::prepare_target_change();
            match protocol::config::Config::transaction(|c| c.activate_device(&id)) {
                Ok(()) => self.set_status("Cihaz seçildi. Görüntü için Ekranı Aç düğmesini kullanın.", Duration::from_secs(5)),
                Err(e) => self.set_status(&format!("Cihaz seçilemedi: {e}"), Duration::from_secs(6)),
            }
            crate::capture::CONFIG_DIRTY.store(true, Ordering::SeqCst);
            crate::android::update_selection(&load_config());
            self.refresh_saved();
            self.refresh_paired_label();
        }

        #[unsafe(method(openSaved:))]
        fn openSaved(&self, _sender: Option<&AnyObject>) {
            let cfg = load_config();
            if self.selected_device().as_deref() == Some(cfg.active_device_id.as_str())
                && cfg.active_device().is_some_and(|d| d.permissions.screen)
            {
                crate::desktop::open(self.mtm());
            }
        }

        #[unsafe(method(changePermissions:))]
        fn changePermissions(&self, _sender: Option<&AnyObject>) {
            let Some(id) = self.selected_device() else { return; };
            let iv = self.ivars();
            let checked = |b: &NSButton| -> bool { unsafe { let v: isize = msg_send![b, state]; v == 1 } };
            let permissions = protocol::devices::Permissions {
                screen: checked(&iv.screen_check), input: checked(&iv.input_check),
                clipboard: checked(&iv.clipboard_check),
            };
            let active = load_config().active_device_id == id;
            if active { crate::desktop::prepare_target_change(); }
            let result = protocol::config::Config::transaction(|c| {
                c.set_device_permissions(&id, permissions)
            });
            if active { crate::capture::CONFIG_DIRTY.store(true, Ordering::SeqCst); }
            if let Err(e) = result { self.set_status(&format!("İzinler kaydedilemedi: {e}"), Duration::from_secs(6)); }
            self.refresh_permissions();
        }
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
            if crate::autostart::set_enabled(state == 1).is_ok() {
                let _ = protocol::config::Config::update(|c| c.startup_configured = true);
            }
            self.sync_autostart();
        }

        // "Pair & Connect" -> hand the selected PC to a worker thread. Everything
        // after this point is network + a human on the other machine, so it must
        // not run on the main thread.
        #[unsafe(method(pairAndConnect:))]
        fn pairAndConnect(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            let Some(peer) = self.selected_peer() else {
                self.set_status("Choose your device from the list first.", Duration::from_secs(5));
                return;
            };
            // Guard against a double click landing two workers on one answer slot.
            if !matches!(*iv.pair_state.lock().unwrap(), PairState::Idle) {
                return;
            }
            *iv.pair_state.lock().unwrap() = PairState::Busy(format!("Contacting {}…", peer.name));
            self.refresh_buttons();
            if !load_config().startup_configured {
                if let Err(e) = crate::autostart::set_enabled(true) {
                    *iv.pair_state.lock().unwrap() = PairState::Idle;
                    self.set_status(&format!("Could not enable start at login: {e}"), Duration::from_secs(8));
                    return;
                }
                let _ = protocol::config::Config::update(|c| c.startup_configured = true);
            }
            spawn_pairing(peer, iv.pair_state.clone());
        }

        // "Unpair" -> forget the PC locally. The PC keeps its key; pairing again
        // needs another approval there.
        #[unsafe(method(unpair:))]
        fn unpair(&self, _sender: Option<&AnyObject>) {
            let Some(id) = self.selected_device() else { return; };
            let active = load_config().active_device_id == id;
            if active { crate::desktop::prepare_target_change(); }
            match protocol::config::Config::transaction(|c| c.forget_device(&id)) {
                Ok(()) => {
                    if active { crate::capture::CONFIG_DIRTY.store(true, Ordering::Relaxed); }
                    self.set_status("Cihaz bu Mac'ten kaldırıldı.", Duration::from_secs(4));
                }
                Err(e) => self.set_status(&format!("Could not unpair: {e}"), Duration::from_secs(6)),
            }
            self.refresh_saved();
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
            self.refresh_saved();
            self.refresh_paired_label();
            self.refresh_hint();
            self.sync_autostart();
            self.refresh_status();
            self.refresh_buttons();
        }
    }
);

impl SettingsController {
    /// Build the window + controls, wire targets, start discovery and the timer.
    fn create(mtm: MainThreadMarker, browse: bool) -> Retained<Self> {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
        let content = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 360.0));
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
        window.setTitle(ns_string!("keyboard-it — Cihazlarım"));
        window.center();

        let label = |text: &NSString, x: f64, y: f64, w: f64| -> Retained<NSTextField> {
            let l = NSTextField::labelWithString(text, mtm);
            l.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, 17.0)));
            l
        };

        let saved_label = label(ns_string!("Cihazlarım"), 16.0, 320.0, 118.0);
        let saved_popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(138.0, 314.0), NSSize::new(486.0, 26.0)),
            false,
        );
        saved_popup.addItemWithTitle(ns_string!("Henüz kayıtlı cihaz yok"));
        saved_popup.setEnabled(false);
        let connect_button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("Bu Cihaza Geç"), None, None, mtm)
        };
        connect_button.setFrame(NSRect::new(
            NSPoint::new(464.0, 270.0),
            NSSize::new(160.0, 32.0),
        ));
        let open_button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("Ekranı Aç"), None, None, mtm)
        };
        open_button.setFrame(NSRect::new(
            NSPoint::new(288.0, 270.0),
            NSSize::new(160.0, 32.0),
        ));
        let screen_check = unsafe {
            NSButton::checkboxWithTitle_target_action(ns_string!("Ekran"), None, None, mtm)
        };
        let input_check = unsafe {
            NSButton::checkboxWithTitle_target_action(ns_string!("Klavye / fare"), None, None, mtm)
        };
        let clipboard_check = unsafe {
            NSButton::checkboxWithTitle_target_action(ns_string!("Metin panosu"), None, None, mtm)
        };
        for (button, x, width) in [
            (&screen_check, 138.0, 100.0),
            (&input_check, 250.0, 150.0),
            (&clipboard_check, 414.0, 180.0),
        ] {
            button.setFrame(NSRect::new(
                NSPoint::new(x, 238.0),
                NSSize::new(width, 24.0),
            ));
        }
        let pc_label = label(ns_string!("Yeni cihaz ekle"), 16.0, 198.0, 118.0);
        let popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(138.0, 192.0), NSSize::new(486.0, 26.0)),
            false,
        );
        // Placeholder until discovery reports something (refresh_popup rebuilds).
        popup.addItemWithTitle(ns_string!("Searching your network…"));
        popup.setEnabled(false);

        let hint = label(
            ns_string!("Compare all 8 fingerprint groups on both devices before Allow."),
            16.0,
            42.0,
            608.0,
        );

        let pair_button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("Pair & Connect"), None, None, mtm)
        };
        pair_button.setFrame(NSRect::new(
            NSPoint::new(464.0, 144.0),
            NSSize::new(150.0, 32.0),
        ));
        let android_button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("Android Ekle…"), None, None, mtm)
        };
        android_button.setFrame(NSRect::new(NSPoint::new(16.0, 144.0), NSSize::new(160.0, 32.0)));
        // Return triggers pairing — the whole flow works keyboard-only.
        pair_button.setKeyEquivalent(ns_string!("\r"));

        let paired_label = label(ns_string!(""), 16.0, 112.0, 608.0);
        let unpair_button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("Cihazı Unut"), None, None, mtm)
        };
        unpair_button.setFrame(NSRect::new(
            NSPoint::new(16.0, 270.0),
            NSSize::new(130.0, 32.0),
        ));

        let autostart_check = unsafe {
            NSButton::checkboxWithTitle_target_action(ns_string!("Start at Login"), None, None, mtm)
        };
        autostart_check.setFrame(NSRect::new(
            NSPoint::new(16.0, 78.0),
            NSSize::new(220.0, 18.0),
        ));

        let status_label = label(ns_string!(""), 16.0, 16.0, 608.0);
        status_label.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(12.0, 0.0)));

        let content_view = window
            .contentView()
            .expect("titled window has a content view");
        for view in [
            &*saved_label,
            &*pc_label,
            &*hint,
            &*paired_label,
            &*status_label,
        ] {
            content_view.addSubview(view);
        }
        content_view.addSubview(&popup);
        content_view.addSubview(&pair_button);
        content_view.addSubview(&android_button);
        content_view.addSubview(&unpair_button);
        content_view.addSubview(&autostart_check);
        content_view.addSubview(&saved_popup);
        for button in [
            &connect_button,
            &open_button,
            &screen_check,
            &input_check,
            &clipboard_check,
        ] {
            content_view.addSubview(&**button);
        }

        // Idempotent: the permissions window may already have started the browse
        // (and with it raised the Local Network prompt) — this is then a no-op.
        if browse {
            discovery::start();
        }

        let this = Self::alloc(mtm).set_ivars(Ivars {
            window,
            saved_popup: saved_popup.clone(),
            shown_devices: RefCell::new(Vec::new()),
            connect_button: connect_button.clone(),
            open_button: open_button.clone(),
            screen_check: screen_check.clone(),
            input_check: input_check.clone(),
            clipboard_check: clipboard_check.clone(),
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
        wire(&android_button, sel!(addAndroid:));
        wire(&unpair_button, sel!(unpair:));
        wire(&connect_button, sel!(connectSaved:));
        wire(&open_button, sel!(openSaved:));
        for check in [&screen_check, &input_check, &clipboard_check] {
            wire(check, sel!(changePermissions:));
        }
        unsafe {
            saved_popup.setTarget(Some(&*this));
            saved_popup.setAction(Some(sel!(savedSelected:)));
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
        self.refresh_saved();
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
            format!(
                "Aktif cihaz: {}",
                cfg.active_device()
                    .map(|d| d.name.as_str())
                    .unwrap_or(&cfg.peer_host)
            )
        } else {
            "Aktif cihaz seçilmedi".to_string()
        };
        let iv = self.ivars();
        if iv.paired_label.stringValue().to_string() != text {
            iv.paired_label.setStringValue(&NSString::from_str(&text));
        }
    }

    fn selected_device(&self) -> Option<String> {
        let iv = self.ivars();
        let index = iv.saved_popup.indexOfSelectedItem();
        if index < 1 {
            return None;
        }
        iv.shown_devices
            .borrow()
            .get((index - 1) as usize)
            .map(|d| d.0.clone())
    }

    fn refresh_permissions(&self) {
        let cfg = load_config();
        let id = self.selected_device();
        let permissions = cfg
            .devices
            .iter()
            .find(|d| Some(&d.id) == id.as_ref())
            .map(|d| &d.permissions);
        let iv = self.ivars();
        for (button, checked) in [
            (&iv.screen_check, permissions.is_some_and(|p| p.screen)),
            (&iv.input_check, permissions.is_some_and(|p| p.input)),
            (
                &iv.clipboard_check,
                permissions.is_some_and(|p| p.clipboard),
            ),
        ] {
            button.setEnabled(permissions.is_some());
            unsafe {
                let _: () = msg_send![&**button, setState: if checked {1isize} else {0isize}];
            }
        }
    }

    fn refresh_saved(&self) {
        let cfg = load_config();
        let peers = discovery::peers();
        let now: Vec<_> = cfg
            .devices
            .iter()
            .map(|d| {
                let online = peers.iter().any(|p| {
                    if !d.peer_device_id.is_empty() {
                        p.device_id == d.peer_device_id
                    } else {
                        p.hostname == d.host || p.host == d.ip
                    }
                });
                let state = if d.is_android() && cfg.active_device_id == d.id {
                    "aktif · Android"
                } else if d.is_android() {
                    "Android · kayıtlı"
                } else if cfg.active_device_id == d.id {
                    "aktif"
                } else if online {
                    "ağda"
                } else {
                    "ağda görünmüyor"
                };
                (d.id.clone(), format!("{} — {}", d.name, state))
            })
            .collect();
        let iv = self.ivars();
        if now != *iv.shown_devices.borrow() {
            let selected = self
                .selected_device()
                .unwrap_or_else(|| cfg.active_device_id.clone());
            iv.saved_popup.removeAllItems();
            iv.saved_popup
                .addItemWithTitle(ns_string!("Kayıtlı bir cihaz seçin…"));
            for (_, title) in &now {
                iv.saved_popup.addItemWithTitle(&NSString::from_str(title));
            }
            iv.saved_popup.setEnabled(!now.is_empty());
            if let Some(i) = now.iter().position(|d| d.0 == selected) {
                iv.saved_popup.selectItemAtIndex((i + 1) as isize);
            }
            *iv.shown_devices.borrow_mut() = now;
            self.refresh_permissions();
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
        iv.pair_button
            .setEnabled(!pairing && self.selected_peer().is_some());
        iv.unpair_button
            .setEnabled(!pairing && self.selected_device().is_some());
        iv.connect_button.setEnabled(
            !pairing
                && self
                    .selected_device()
                    .is_some_and(|id| id != load_config().active_device_id),
        );
        let cfg = load_config();
        iv.open_button.setEnabled(
            !pairing
                && self.selected_device().as_deref() == Some(cfg.active_device_id.as_str())
                && cfg.active_device().is_some_and(|d| d.permissions.screen),
        );
        for check in [&iv.screen_check, &iv.input_check, &iv.clipboard_check] {
            let cfg = load_config();
            let selected = self.selected_device();
            let android = cfg.devices.iter().any(|d| Some(&d.id) == selected.as_ref() && d.is_android());
            check.setEnabled(!pairing && selected.is_some() && !(std::ptr::eq(&**check, &*iv.clipboard_check) && android));
        }
    }

    /// True once the browse has run long enough with nothing to show that
    /// "Searching your network…" has stopped being an honest answer.
    fn discovery_stalled(&self) -> bool {
        discovery::stalled()
    }

    /// The line under the popup: what to do next, or why there is nothing to do.
    fn refresh_hint(&self) {
        let iv = self.ivars();
        let cfg = load_config();
        let selected = self.selected_device();
        let caps = cfg
            .devices
            .iter()
            .find(|d| Some(&d.id) == selected.as_ref())
            .and_then(|d| d.capabilities.as_ref());
        let android = cfg.devices.iter().any(|d| Some(&d.id) == selected.as_ref() && d.is_android());
        let text = if android {
            "Android: Ekranı Aç · giriş izni kapalıysa yalnız izleme · otomatik pano kapalı.".to_string()
        } else if let Some(caps) = caps {
            let mut functions = Vec::new();
            if caps.capture_screen && caps.encode_h264 {
                functions.push("ekran");
            }
            if caps.receive_input {
                functions.push("klavye/fare");
            }
            if caps.clipboard {
                functions.push("pano");
            }
            if functions.is_empty() {
                "Cihaz ekran, giriş veya pano desteği bildirmedi.".to_string()
            } else {
                format!("Cihazın bildirdiği işlevler: {}", functions.join(", "))
            }
        } else if self.discovery_stalled() {
            "Nothing is answering on this network.".to_string()
        } else {
            "Compare all 8 fingerprint groups on both devices before Allow.".to_string()
        };
        if iv.hint_label.stringValue().to_string() != text {
            iv.hint_label.setStringValue(&NSString::from_str(&text));
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
        let disk: isize = if !load_config().startup_configured || crate::autostart::is_enabled() {
            1
        } else {
            0
        };
        let shown: isize = unsafe { msg_send![&*iv.autostart_check, state] };
        if shown != disk {
            unsafe {
                let _: () = msg_send![&*iv.autostart_check, setState: disk];
            }
        }
    }

    /// Drive the status line. Pairing progress outranks the connection state:
    /// while a request is in flight, the complete pairing fingerprint is
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
                PairState::AwaitingApproval {
                    peer,
                    code,
                    deadline,
                } => {
                    let left = deadline.saturating_duration_since(Instant::now()).as_secs();
                    iv.hint_label.setStringValue(&NSString::from_str(&format!(
                        "{peer}: compare ALL 8 groups below, then Allow ({left}s)")));
                    // Keep the complete fingerprint visible; names/countdowns
                    // must never truncate the security comparison in this label.
                    Some(code.clone())
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
        if load_config().android_selected() {
            let status = crate::android::status();
            iv.status_label.setStringValue(&NSString::from_str(if status.message.is_empty() { "Android seçildi — Ekranı Aç" } else { &status.message }));
            return;
        }
        let Some(src) = CONN_STATUS.get() else { return };
        let conn = ConnStatus::from_u8(src.load(Ordering::Relaxed));
        // A stalled browse outranks the connection line. "No connection —
        // retrying in the background" is true, but it describes a link that
        // cannot be established while the real problem is that the user has no
        // way to pick a PC in the first place. Say the thing they can act on.
        let text = if conn != ConnStatus::Connected && self.discovery_stalled() {
            "No devices found — check Privacy & Security \u{203A} Local Network."
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
                "No devices found"
            } else {
                "Searching your network…"
            };
            // Compare against item 0 rather than the selected item: whether a
            // freshly repopulated popup auto-selects anything is not something to
            // depend on, and if it did not, this would rebuild on every tick.
            let have = (popup.numberOfItems() == 1).then(|| popup.itemTitleAtIndex(0).to_string());
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

/// Main-thread integration check with an isolated config and hidden controls.
/// Does not discover devices, request OS permissions, capture or inject input.
#[cfg(debug_assertions)]
pub fn check_devices() -> std::io::Result<()> {
    use protocol::{
        config::Config,
        devices::{PairedDevice, Platform},
    };
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| std::io::Error::other("AppKit requires main thread"))?;
    let _app = NSApplication::sharedApplication(mtm);
    crate::host::check_ui(mtm);
    let dir = std::env::temp_dir().join(format!(
        "keyboard-it-ui-{}",
        protocol::secure::generate_key()
    ));
    std::fs::create_dir_all(&dir)?;
    let previous = std::env::var_os("KEYBOARD_IT_CONFIG");
    struct Cleanup(std::path::PathBuf, Option<std::ffi::OsString>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(value) = &self.1 {
                std::env::set_var("KEYBOARD_IT_CONFIG", value);
            } else {
                std::env::remove_var("KEYBOARD_IT_CONFIG");
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone(), previous);
    std::env::set_var("KEYBOARD_IT_CONFIG", dir.join("config.toml"));
    let mut cfg = Config::default();
    for id in ["a", "b"] {
        let capabilities = (id == "b").then(|| {
            let mut caps = protocol::capabilities::Capabilities::host(Platform::Windows);
            caps.receive_input = false;
            caps.clipboard = false;
            caps
        });
        cfg.remember_device(PairedDevice {
            transport: Default::default(),
            capabilities,
            id: id.into(),
            name: format!("Test PC {id}"),
            platform: Platform::Windows,
            permissions: Default::default(),
            peer_device_id: id.into(),
            shared_secret: format!("test-key-{id}"),
            host: format!("{id}.invalid"),
            ip: String::new(),
            port: 5599,
            desktop_port: 5601,
        })?;
    }
    cfg.save()?;
    let controller = SettingsController::create(mtm, false);
    controller.load_fields_from_config();
    let iv = controller.ivars();
    assert!(!iv.window.isVisible());
    assert_eq!(iv.saved_popup.numberOfItems(), 3);
    // A clipped fingerprint silently weakens human verification. Exercise the
    // actual native label with the widest hexadecimal glyphs, not a mock view.
    let fingerprint = protocol::pairing::code_display(&"D".repeat(64));
    *iv.pair_state.lock().unwrap() = PairState::AwaitingApproval {
        peer: "Fixture PC".into(), code: fingerprint.clone(),
        deadline: Instant::now() + Duration::from_secs(120),
    };
    controller.refresh_status();
    assert_eq!(iv.status_label.stringValue().to_string(), fingerprint);
    let needed: NSSize = unsafe { msg_send![&*iv.status_label, intrinsicContentSize] };
    assert!(needed.width <= iv.status_label.frame().size.width,
        "pairing fingerprint would be clipped: {} > {}", needed.width, iv.status_label.frame().size.width);
    *iv.pair_state.lock().unwrap() = PairState::Idle;
    assert_eq!(controller.selected_device().as_deref(), Some("a"));
    iv.saved_popup.selectItemAtIndex(2);
    controller.savedSelected(sel!(savedSelected:), None);
    assert_eq!(controller.selected_device().as_deref(), Some("b"));
    let hint = iv.hint_label.stringValue().to_string();
    assert!(hint.contains("ekran") && !hint.contains("klavye/fare") && !hint.contains("pano"));
    assert!(iv.connect_button.isEnabled());
    assert!(!iv.open_button.isEnabled());
    crate::capture::CONFIG_DIRTY.store(false, Ordering::SeqCst);
    unsafe {
        let _: () = msg_send![&*iv.clipboard_check, setState: 0isize];
    }
    controller.changePermissions(sel!(changePermissions:), None);
    assert!(!crate::capture::CONFIG_DIRTY.load(Ordering::SeqCst));
    assert!(
        !Config::load()?
            .unwrap()
            .devices
            .iter()
            .find(|d| d.id == "b")
            .unwrap()
            .permissions
            .clipboard
    );
    controller.unpair(sel!(unpair:), None); // inactive target: exercises actual persistence and UI refresh
    assert!(!crate::capture::CONFIG_DIRTY.load(Ordering::SeqCst));
    assert_eq!(Config::load()?.unwrap().active_device_id, "a");
    assert_eq!(iv.saved_popup.numberOfItems(), 2);
    // Android setup and registry selection are tested without running adb/scrcpy.
    crate::android::check_setup(mtm);
    Config::transaction(|c| {
        c.remember_device(PairedDevice {
            transport: protocol::devices::DeviceTransport::Adb { serial: "fixture-android".into(), android_id: "1234567890abcdef".into() },
            capabilities: None, id: "android".into(), name: "Test Android".into(), platform: Platform::Android,
            permissions: protocol::devices::Permissions { screen: true, input: false, clipboard: false },
            peer_device_id: String::new(), shared_secret: String::new(), host: String::new(), ip: String::new(), port: 5599, desktop_port: 5601,
        })?;
        Ok(())
    })?;
    controller.refresh_saved();
    iv.saved_popup.selectItemAtIndex(2);
    controller.savedSelected(sel!(savedSelected:), None);
    assert!(!iv.clipboard_check.isEnabled());
    controller.connectSaved(sel!(connectSaved:), None);
    let android = Config::load()?.unwrap();
    assert!(android.android_selected() && android.shared_secret.is_empty() && android.peer_addrs().is_empty());
    assert!(crate::android::SELECTED.load(Ordering::SeqCst));
    assert!(!crate::android::status().running);
    controller.refresh_buttons();
    assert!(iv.open_button.isEnabled());
    Config::transaction(|c| {
        c.devices.iter_mut().find(|d| d.id == "android").unwrap().permissions.screen = false;
        Ok(())
    })?;
    controller.refresh_buttons();
    assert!(!iv.open_button.isEnabled());
    for control in [
        &iv.connect_button,
        &iv.open_button,
        &iv.pair_button,
        &iv.unpair_button,
        &iv.screen_check,
        &iv.input_check,
        &iv.clipboard_check,
    ] {
        let frame = control.frame();
        assert!(frame.origin.x >= 0.0 && frame.origin.y >= 0.0);
        assert!(
            frame.origin.x + frame.size.width <= 640.0
                && frame.origin.y + frame.size.height <= 360.0
        );
    }
    println!("Devices UI check passed (two PCs and Android setup/selection; no network, ADB or physical input)");
    Ok(())
}

// The Edit menu that used to live here existed solely so Cmd+V worked in the
// pairing-key field. There are no text fields left, so it went with them.

fn load_config() -> protocol::config::Config {
    protocol::config::Config::load()
        .ok()
        .flatten()
        .unwrap_or_default()
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
                format!(
                    "{} is running an older keyboard-it — update it on the PC.",
                    peer.name
                ),
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
        // Show the handshake-derived comparison code during the one-time
        // approval, and extend the socket deadline while the user checks it.
        let identity = load_config().device_id;
        let result = protocol::pairing::pair_initiator_with_identity(
            &mut stream,
            &name,
            &identity,
            |code| {
                if let Some(c) = &ctl {
                    let _ = c.set_read_timeout(Some(PAIR_DECISION_WAIT));
                }
                if let Ok(mut g) = asked_at_cb.lock() {
                    *g = Some(Instant::now());
                }
                set(PairState::AwaitingApproval {
                    peer: peer.name.clone(),
                    code: protocol::pairing::code_display(code),
                    deadline: Instant::now() + protocol::PAIR_DECISION_TIMEOUT,
                });
            },
        );

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
                    done(
                        format!("No answer on {} — the request expired.", peer.name),
                        10,
                    );
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

        let host = if peer.hostname.is_empty() {
            peer.host.clone()
        } else {
            peer.hostname.clone()
        };
        let result = protocol::config::Config::transaction(|cfg| {
            let previous_target = cfg.session_target();
            cfg.role = protocol::config::Role::Sender;
            cfg.remember_device(protocol::devices::PairedDevice {
                transport: Default::default(),
                capabilities: None,
                id: String::new(),
                name: outcome.peer_name.clone(),
                platform: protocol::devices::Platform::Unknown,
                permissions: Default::default(),
                // Identity comes from the approved encrypted pairing, never mDNS.
                peer_device_id: outcome.peer_device_id,
                shared_secret: outcome.secret,
                host,
                ip: peer.host.clone(),
                port: outcome.session_port,
                desktop_port: peer.desktop_port.unwrap_or(protocol::desktop::DEFAULT_PORT),
            })?;
            Ok(cfg.session_target() != previous_target)
        });
        let changed_target = match result {
            Ok(changed) => changed,
            Err(e) => {
                done(
                    format!("Paired, but the settings could not be saved: {e}"),
                    10,
                );
                return;
            }
        };
        // The connection thread polls this (~1 s) and reconnects with the new
        // key and address — no restart.
        if changed_target {
            crate::capture::release_input();
            crate::desktop::shared().invalidate_target();
            crate::capture::CONFIG_DIRTY.store(true, Ordering::SeqCst);
        }
        done(
            format!(
                "{} kaydedildi. Cihazlarım listesinden seçebilirsiniz.",
                outcome.peer_name
            ),
            5,
        );
    });
}
