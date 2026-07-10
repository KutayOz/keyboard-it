//! Native settings window — the zero-terminal replacement for editing config.toml.
//!
//! Pairing key (with Generate), a "Your Windows PC" popup fed by mDNS discovery,
//! manual host/port fields, Start at Login, Save, and a live status line. Save
//! writes the config and flips capture::CONFIG_DIRTY so the background connection
//! drops and reconnects with the new values within about a second — no restart.
//!
//! Threading: AppKit objects are main-thread-only. The mDNS browser runs on a
//! background thread and only writes an Arc<Mutex<Vec<DiscoveredPeer>>>; a 1 s
//! main-thread NSTimer mirrors that list into the popup (same pattern as
//! menubar::install_status_updater).

#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSMenu, NSMenuItem, NSPopUpButton, NSTextField,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSTimer,
};

use crate::menubar::ConnStatus;

/// One receiver found via mDNS (win-receiver advertises protocol::MDNS_SERVICE).
/// `fullname` is the dedupe/removal key (ServiceRemoved only carries the fullname).
#[derive(Clone, PartialEq, Eq)]
struct DiscoveredPeer {
    fullname: String,
    name: String,
    host: String,
    port: u16,
}

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

fn conn_status_text(s: ConnStatus) -> &'static str {
    match s {
        ConnStatus::ConfigNeeded => "Waiting for setup — enter a pairing key and choose a PC.",
        ConnStatus::Connecting => "Connecting…",
        ConnStatus::Connected => "Connected.",
        ConnStatus::Disconnected => "No connection — retrying in the background.",
        ConnStatus::HandshakeFailed => "Key mismatch — use the same pairing key on both sides.",
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
    key_field: Retained<NSTextField>,
    host_field: Retained<NSTextField>,
    port_field: Retained<NSTextField>,
    popup: Retained<NSPopUpButton>,
    autostart_check: Retained<NSButton>,
    status_label: Retained<NSTextField>,
    /// Written by the mDNS browser thread, read by the 1 s timer.
    peers: Arc<Mutex<Vec<DiscoveredPeer>>>,
    /// The list currently rendered in the popup; popupSelected: indexes into
    /// THIS (not `peers`) so a refresh between click and action cannot drift.
    shown_peers: RefCell<Vec<DiscoveredPeer>>,
    /// While set and in the future, the timer must not overwrite the status line
    /// (save feedback/errors would otherwise vanish within a second).
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
        // "Generate" -> fresh pairing key into the field (not saved until Save).
        #[unsafe(method(generateKey:))]
        fn generateKey(&self, _sender: Option<&AnyObject>) {
            let key = protocol::secure::generate_key();
            self.ivars().key_field.setStringValue(&NSString::from_str(&key));
        }

        // Popup selection fills the manual host/port fields; Save always reads the
        // FIELDS, so whichever the user touched last (popup or typing) wins.
        #[unsafe(method(popupSelected:))]
        fn popupSelected(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            let idx = iv.popup.indexOfSelectedItem();
            if idx < 1 {
                return; // index 0 is the placeholder row
            }
            let peers = iv.shown_peers.borrow();
            if let Some(p) = peers.get((idx - 1) as usize) {
                iv.host_field.setStringValue(&NSString::from_str(&p.host));
                iv.port_field.setStringValue(&NSString::from_str(&p.port.to_string()));
            }
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

        // Save -> config.toml + wake the connection thread so it reconnects now.
        #[unsafe(method(save:))]
        fn save(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            let key = iv.key_field.stringValue().to_string().trim().to_string();
            let host = iv.host_field.stringValue().to_string().trim().to_string();
            let port_text = iv.port_field.stringValue().to_string().trim().to_string();
            let port = if port_text.is_empty() {
                protocol::DEFAULT_PORT
            } else {
                match port_text.parse::<u16>() {
                    Ok(p) if p > 0 => p,
                    _ => {
                        self.set_status(
                            "Port must be a number between 1 and 65535.",
                            Duration::from_secs(6),
                        );
                        return;
                    }
                }
            };
            let cfg = protocol::config::Config {
                shared_secret: key,
                peer_host: host,
                role: protocol::config::Role::Sender,
                port,
            };
            match cfg.save() {
                Ok(()) => {
                    // The connection thread polls this flag (~1 s) and drops the
                    // current connection, so the new address/key applies at once.
                    crate::capture::CONFIG_DIRTY.store(true, Ordering::Relaxed);
                    self.set_status("Saved — applying…", Duration::from_secs(2));
                }
                Err(e) => {
                    self.set_status(&format!("Save failed: {e}"), Duration::from_secs(6));
                }
            }
        }

        // 1 s heartbeat while the window is visible: discovery list -> popup,
        // autostart state -> checkbox, connection state -> status line.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            if !self.ivars().window.isVisible() {
                return;
            }
            self.refresh_popup();
            self.sync_autostart();
            self.refresh_status();
        }
    }
);

impl SettingsController {
    /// Build the window + controls, wire targets, start discovery and the timer.
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        // Cmd+V/C/X/A route through the main menu, which an Accessory app does
        // not have — install a minimal Edit menu so pasting the pairing key works.
        install_edit_menu(mtm);

        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
        let content = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(440.0, 226.0));
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
        let field = |x: f64, y: f64, w: f64| -> Retained<NSTextField> {
            let f = NSTextField::textFieldWithString(ns_string!(""), mtm);
            f.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, 24.0)));
            f
        };

        let key_label = label(ns_string!("Pairing key"), 16.0, 191.0, 118.0);
        let key_field = field(140.0, 187.0, 172.0);
        key_field.setPlaceholderString(Some(ns_string!("same key as on Windows")));
        let generate = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!("Generate"), None, None, mtm)
        };
        generate.setFrame(NSRect::new(NSPoint::new(316.0, 182.0), NSSize::new(108.0, 32.0)));

        let pc_label = label(ns_string!("Your Windows PC"), 16.0, 155.0, 118.0);
        let popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(138.0, 149.0), NSSize::new(286.0, 26.0)),
            false,
        );
        // Placeholder until discovery reports something (refresh_popup rebuilds).
        popup.addItemWithTitle(ns_string!("Searching your network…"));
        popup.setEnabled(false);

        let host_label = label(ns_string!("Host / IP"), 16.0, 119.0, 118.0);
        let host_field = field(140.0, 115.0, 164.0);
        host_field.setPlaceholderString(Some(ns_string!("e.g. 192.168.1.20")));
        let port_label = label(ns_string!("Port"), 310.0, 119.0, 36.0);
        let port_field = field(348.0, 115.0, 76.0);

        let autostart_check = unsafe {
            NSButton::checkboxWithTitle_target_action(ns_string!("Start at Login"), None, None, mtm)
        };
        autostart_check.setFrame(NSRect::new(NSPoint::new(138.0, 85.0), NSSize::new(220.0, 18.0)));

        let status_label = label(ns_string!(""), 16.0, 20.0, 294.0);
        let save =
            unsafe { NSButton::buttonWithTitle_target_action(ns_string!("Save"), None, None, mtm) };
        save.setFrame(NSRect::new(NSPoint::new(316.0, 12.0), NSSize::new(108.0, 32.0)));
        // Return key triggers Save — the whole flow works keyboard-only.
        save.setKeyEquivalent(ns_string!("\r"));

        let content_view = window.contentView().expect("titled window has a content view");
        for view in [
            &*key_label, &*key_field, &*host_label, &*host_field, &*port_label, &*port_field,
            &*pc_label, &*status_label,
        ] {
            content_view.addSubview(view);
        }
        content_view.addSubview(&generate);
        content_view.addSubview(&popup);
        content_view.addSubview(&autostart_check);
        content_view.addSubview(&save);

        let peers = Arc::new(Mutex::new(Vec::new()));
        spawn_browser(peers.clone());

        let this = Self::alloc(mtm).set_ivars(Ivars {
            window,
            key_field,
            host_field,
            port_field,
            popup: popup.clone(),
            autostart_check: autostart_check.clone(),
            status_label,
            peers,
            shown_peers: RefCell::new(Vec::new()),
            status_hold: Cell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Targets are wired after init: the controls must exist before the
        // controller (they live in its ivars), so they start target-less.
        let wire = |control: &NSButton, action: Sel| unsafe {
            control.setTarget(Some(&*this));
            control.setAction(Some(action));
        };
        wire(&generate, sel!(generateKey:));
        wire(&autostart_check, sel!(toggleAutostart:));
        wire(&save, sel!(save:));
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

    /// Populate the fields from config.toml (called when the window opens).
    fn load_fields_from_config(&self) {
        let cfg = protocol::config::Config::load().ok().flatten().unwrap_or_default();
        let iv = self.ivars();
        iv.key_field.setStringValue(&NSString::from_str(&cfg.shared_secret));
        iv.host_field.setStringValue(&NSString::from_str(&cfg.peer_host));
        iv.port_field.setStringValue(&NSString::from_str(&cfg.port.to_string()));
        self.sync_autostart();
        self.refresh_status();
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

    /// Mirror the connection state into the status line (unless save feedback
    /// is being held on screen).
    fn refresh_status(&self) {
        let iv = self.ivars();
        if let Some(until) = iv.status_hold.get() {
            if Instant::now() < until {
                return;
            }
            iv.status_hold.set(None);
        }
        let Some(src) = CONN_STATUS.get() else { return };
        let text = conn_status_text(ConnStatus::from_u8(src.load(Ordering::Relaxed)));
        if iv.status_label.stringValue().to_string() != text {
            iv.status_label.setStringValue(&NSString::from_str(text));
        }
    }

    /// Rebuild the popup when the discovered list changed since the last render.
    fn refresh_popup(&self) {
        let iv = self.ivars();
        let mut now: Vec<DiscoveredPeer> =
            iv.peers.lock().map(|g| g.clone()).unwrap_or_default();
        // Stable order: the browser thread appends in resolve order, which would
        // make entries jump around between refreshes.
        now.sort_by(|a, b| a.name.cmp(&b.name));
        if now == *iv.shown_peers.borrow() {
            return;
        }
        let popup = &iv.popup;
        let selected = popup.titleOfSelectedItem();
        popup.removeAllItems();
        if now.is_empty() {
            popup.addItemWithTitle(ns_string!("Searching your network…"));
            popup.setEnabled(false);
        } else {
            popup.setEnabled(true);
            popup.addItemWithTitle(ns_string!("Choose a discovered PC…"));
            for p in &now {
                let title = NSString::from_str(&format!("{} ({}:{})", p.name, p.host, p.port));
                popup.addItemWithTitle(&title);
            }
            // Keep the user's selection across rebuilds when it still exists.
            if let Some(title) = selected {
                let idx = popup.indexOfItemWithTitle(&title);
                if idx >= 0 {
                    popup.selectItemAtIndex(idx);
                }
            }
        }
        *iv.shown_peers.borrow_mut() = now;
    }
}

/// Accessory apps have no main menu, so Cmd+V/C/X/Z/A do nothing in text fields
/// (key equivalents resolve through the main menu). A minimal Edit menu is never
/// visible for a menu-bar-less app but makes paste work — essential for a key
/// copied from the Windows side. nil targets mean "first responder", i.e. the
/// focused field editor.
fn install_edit_menu(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    if app.mainMenu().is_some() {
        return;
    }
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Edit"));
    let add = |title: &NSString, action: Sel, key: &NSString| {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                title,
                Some(action),
                key,
            )
        };
        edit.addItem(&item);
    };
    add(ns_string!("Undo"), sel!(undo:), ns_string!("z"));
    add(ns_string!("Cut"), sel!(cut:), ns_string!("x"));
    add(ns_string!("Copy"), sel!(copy:), ns_string!("c"));
    add(ns_string!("Paste"), sel!(paste:), ns_string!("v"));
    add(ns_string!("Select All"), sel!(selectAll:), ns_string!("a"));

    let main = NSMenu::new(mtm);
    let edit_item = NSMenuItem::new(mtm);
    edit_item.setSubmenu(Some(&edit));
    main.addItem(&edit_item);
    app.setMainMenu(Some(&main));
}

/// Browse protocol::MDNS_SERVICE in the background for the window's lifetime.
/// Only the shared Vec is touched from here — AppKit stays on the main thread.
fn spawn_browser(peers: Arc<Mutex<Vec<DiscoveredPeer>>>) {
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
                    // Prefer IPv4 (recognizable to users); min() keeps the pick
                    // stable across re-resolves so the popup does not churn.
                    let addrs: Vec<std::net::IpAddr> =
                        info.get_addresses().iter().map(|a| a.to_ip_addr()).collect();
                    let ip = addrs
                        .iter()
                        .filter(|a| a.is_ipv4())
                        .min()
                        .or_else(|| addrs.iter().min())
                        .copied();
                    let Some(ip) = ip else { continue };
                    let fullname = info.get_fullname().to_string();
                    let name = fullname
                        .strip_suffix(protocol::MDNS_SERVICE)
                        .map(|s| s.trim_end_matches('.').to_string())
                        .unwrap_or_else(|| fullname.clone());
                    let peer = DiscoveredPeer {
                        fullname: fullname.clone(),
                        name,
                        host: ip.to_string(),
                        port: info.get_port(),
                    };
                    if let Ok(mut list) = peers.lock() {
                        match list.iter_mut().find(|p| p.fullname == fullname) {
                            Some(existing) => *existing = peer,
                            None => list.push(peer),
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_ty, fullname) => {
                    if let Ok(mut list) = peers.lock() {
                        list.retain(|p| p.fullname != fullname);
                    }
                }
                _ => {}
            }
        }
    });
}
