//! macOS menu bar (status bar) indicator: INACTIVE/ACTIVE + Quit.
//!
//! - Accessory activation policy → no Dock icon, menu bar only.
//! - The NSStatusItem title shows the state as an emoji + text.
//! - The dropdown NSMenu has "Quit" → re-associates the cursor before exiting.
//!
//! Everything runs on the MAIN thread (run() is called from the main thread and the tap
//! callback fires there too), so updating the title directly from the callback is safe.

#![allow(non_snake_case)]

use std::cell::Cell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSTimer,
};

/// Connection state — parity with win-receiver serve::ConnStatus
/// (Connected/Disconnected/HandshakeFailed) plus the sender-specific Connecting and
/// ConfigNeeded. Carried in an AtomicU8: the background connection thread writes it,
/// a main-thread timer reads it and reflects it in the menu bar title.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ConnStatus {
    /// Pairing key and/or peer_host is empty — must be filled in via 'Settings...'.
    ConfigNeeded = 0,
    /// TCP connection to the peer is being attempted.
    Connecting = 1,
    /// Encrypted channel established.
    Connected = 2,
    /// Connection closed/dropped; retrying in the background.
    Disconnected = 3,
    /// Handshake rejected — most likely the keys differ between the two sides
    /// (secure.rs reports this as the peer rejecting the handshake).
    HandshakeFailed = 4,
}

impl ConnStatus {
    /// Convert back from an AtomicU8 value (unknown value = ConfigNeeded, the most harmless).
    pub fn from_u8(v: u8) -> ConnStatus {
        match v {
            1 => ConnStatus::Connecting,
            2 => ConnStatus::Connected,
            3 => ConnStatus::Disconnected,
            4 => ConnStatus::HandshakeFailed,
            _ => ConnStatus::ConfigNeeded,
        }
    }
}

/// Menu bar title (emoji + text) for INACTIVE/ACTIVE plus the connection state, so a
/// dropped connection is visible from the menu bar.
pub fn title_for(active: bool, conn: ConnStatus) -> &'static NSString {
    match (conn, active) {
        (ConnStatus::ConfigNeeded, _) => ns_string!("\u{2699}\u{FE0F} Setup needed"), // gear
        (ConnStatus::HandshakeFailed, _) => ns_string!("\u{1F511} Key mismatch"), // key
        (ConnStatus::Connecting, false) => ns_string!("\u{23F3} Connecting…"), // hourglass
        (ConnStatus::Connecting, true) => ns_string!("\u{26A0}\u{FE0F} ACTIVE (connecting…)"), // warning
        (ConnStatus::Disconnected, false) => ns_string!("\u{1F50C} No connection"), // plug
        (ConnStatus::Disconnected, true) => ns_string!("\u{26A0}\u{FE0F} ACTIVE (no connection)"), // warning
        (ConnStatus::Connected, true) => ns_string!("\u{1F7E2} ACTIVE"), // green circle
        (ConnStatus::Connected, false) => ns_string!("\u{1F512} INACTIVE"), // lock
    }
}

/// Modal info dialog (NSAlert). In an LSUIElement .app stderr/stdout are invisible —
/// this is the only visible way to tell the user about first-run setup, permissions,
/// or errors.
pub fn show_alert(mtm: MainThreadMarker, title: &str, text: &str) {
    // In an Accessory (no Dock) app the dialog can stay behind other windows: bring it forward.
    let app = NSApplication::sharedApplication(mtm);
    unsafe {
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    let _ = alert.runModal();
}

/// Two-button dialog: 'Open Settings' / 'Later'. Returns true when the user chose to
/// open settings; the caller decides WHAT to open (the native settings window for
/// first-run setup, the System Settings Input Monitoring pane for the permission dialog).
pub fn show_setup_alert(mtm: MainThreadMarker, title: &str, text: &str) -> bool {
    let app = NSApplication::sharedApplication(mtm);
    unsafe {
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    let _ = alert.addButtonWithTitle(ns_string!("Open Settings"));
    let _ = alert.addButtonWithTitle(ns_string!("Later"));
    // The first button added = NSAlertFirstButtonReturn.
    alert.runModal() == objc2_app_kit::NSAlertFirstButtonReturn
}

/// N-button modal dialog; buttons[0] is the default (Return key). Returns the
/// 0-based index of the clicked button. The permission wizard builds its
/// Continue/check/Later chains from this.
pub fn show_choice_alert(mtm: MainThreadMarker, title: &str, text: &str, buttons: &[&str]) -> usize {
    let app = NSApplication::sharedApplication(mtm);
    unsafe {
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    for b in buttons {
        let _ = alert.addButtonWithTitle(&NSString::from_str(b));
    }
    // Buttons answer NSAlertFirstButtonReturn + index, in the order they were added.
    (alert.runModal() - objc2_app_kit::NSAlertFirstButtonReturn).max(0) as usize
}

/// While ACTIVE, CGAssociateMouseAndMouseCursorPosition(0) freezes the cursor.
/// Without re-associating on exit, the cursor stays frozen system-wide.
fn reassociate_cursor() {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
    }
    unsafe {
        let _ = CGAssociateMouseAndMouseCursorPosition(1);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KbItQuitDelegate"]
    struct QuitDelegate;

    unsafe impl NSObjectProtocol for QuitDelegate {}

    unsafe impl NSApplicationDelegate for QuitDelegate {
        // terminate: (Quit menu item) fires on the main thread right before exit.
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            reassociate_cursor();
        }
    }

    impl QuitDelegate {
        // Menu "Quit" -> NSApp.terminate (triggers the applicationWillTerminate cleanup).
        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let mtm = self.mtm();
            NSApplication::sharedApplication(mtm).terminate(None);
        }

        // Menu "Settings..." -> the native settings window (settings.rs). The old
        // config.toml-in-a-text-editor path (Config::edit) is gone from the menu:
        // a non-technical user never has to see the file.
        #[unsafe(method(settings:))]
        fn settings(&self, _sender: Option<&AnyObject>) {
            crate::settings::open(self.mtm());
        }

        // Menu "Start at Login" -> toggle the LaunchAgent and update the checkmark.
        // sender = the clicked NSMenuItem; setState reflects the checkmark.
        #[unsafe(method(toggleStartup:))]
        fn toggle_startup(&self, sender: Option<&AnyObject>) {
            let now = crate::autostart::is_enabled();
            let _ = crate::autostart::set_enabled(!now);
            let enabled = crate::autostart::is_enabled();
            if let Some(item) = sender {
                // NSControlStateValue: On=1, Off=0.
                let state: isize = if enabled { 1 } else { 0 };
                unsafe {
                    let _: () = msg_send![item, setState: state];
                }
            }
        }
    }
);

impl QuitDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Result of the menu bar setup. The caller must keep the status item and delegate
/// alive without dropping them — if dropped, the item disappears from the menu bar.
pub struct MenuBar {
    pub status_item: Retained<NSStatusItem>,
    _delegate: Retained<QuitDelegate>,
}

/// Make NSApplication an Accessory app, install the status item + menu, and set the
/// initial title. Does NOT call app.run() — the caller does that after tap setup.
pub fn setup(mtm: MainThreadMarker, initial_active: bool, initial_conn: ConnStatus) -> MenuBar {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let bar = NSStatusBar::systemStatusBar();
    let status_item = bar.statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status_item.button(mtm) {
        // Initial state: the background thread establishes the connection; when the
        // config is incomplete the caller passes ConfigNeeded ('Setup needed' shows).
        button.setTitle(title_for(initial_active, initial_conn));
    }

    let delegate = QuitDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let menu = NSMenu::new(mtm);
    let settings = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Settings..."),
            Some(sel!(settings:)),
            ns_string!(","),
        )
    };
    unsafe { settings.setTarget(Some(&delegate)) };
    menu.addItem(&settings);

    let startup = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Start at Login"),
            Some(sel!(toggleStartup:)),
            ns_string!(""),
        )
    };
    unsafe { startup.setTarget(Some(&delegate)) };
    unsafe {
        // Reflect the real state (LaunchAgent present or not) as the checkmark at startup.
        let state: isize = if crate::autostart::is_enabled() { 1 } else { 0 };
        let _: () = msg_send![&*startup, setState: state];
    }
    menu.addItem(&startup);

    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Quit"),
            Some(sel!(quit:)),
            ns_string!("q"),
        )
    };
    unsafe { quit.setTarget(Some(&delegate)) };
    menu.addItem(&quit);
    status_item.setMenu(Some(&menu));

    MenuBar {
        status_item,
        _delegate: delegate,
    }
}

/// Periodically updates the menu bar title from the `active` + `conn` (+ permission)
/// state. The tap callback and connection thread must be Send (objc2 objects are !Send
/// and cannot move there), so the title is updated from this MAIN-thread timer.
pub fn install_status_updater(
    mtm: MainThreadMarker,
    status_item: Retained<NSStatusItem>,
    active: Arc<AtomicBool>,
    conn: Arc<AtomicU8>,
    permission_needed: Arc<AtomicBool>,
) {
    // None = write once on the first tick, whatever the initial title from setup() was.
    let last: Cell<Option<(bool, u8, bool)>> = Cell::new(None);
    let block = RcBlock::new(move |_t: NonNull<NSTimer>| {
        let now = (
            active.load(Ordering::Relaxed),
            conn.load(Ordering::Relaxed),
            permission_needed.load(Ordering::Relaxed),
        );
        if last.get() != Some(now) {
            last.set(Some(now));
            if let Some(button) = status_item.button(mtm) {
                // Without the permission nothing can be captured — it overrides every other state.
                let title = if now.2 {
                    ns_string!("\u{26D4} Permission needed") // no-entry sign
                } else {
                    title_for(now.0, ConnStatus::from_u8(now.1))
                };
                button.setTitle(title);
            }
        }
    });
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.15, true, &block);
    }
}
