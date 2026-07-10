//! macOS keyboard capture + double-tap-Fn toggle (M3).
//!
//! State machine:
//!   - INACTIVE (initial): keys work normally on the Mac and are NOT sent to Windows.
//!   - ACTIVE: every key is translated to HID, sent to Windows, AND suppressed on the Mac (Drop).
//! Toggle: press Fn twice within ~400 ms (double-tap).
//!
//! PERMISSIONS (both required):
//!   - Input Monitoring: to observe events.
//!   - Accessibility: to suppress keys (Drop) while ACTIVE.
//! On first run a guided wizard (permission_wizard) walks through both: it fires the
//! official system prompts and relaunches the app after a grant (CGEventTap evaluates
//! permissions at process start). When both are already granted no dialog appears.
//! PREREQUISITE: System Settings > Keyboard > set "Press fn (globe) key to" to "Do Nothing"
//!   (otherwise macOS may reserve double-Fn for Dictation and swallow the toggle).
//!
//! SAFETY: while ACTIVE the Mac keyboard is suppressed; if you get stuck, the mouse still
//! works —  menu > Force Quit. Double-tap Fn always returns to INACTIVE.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

use protocol::{mousebtn, InputEvent, KeyEvent, MsgType};

use crate::keymap::mac_keycode_to_hid;
use crate::menubar::{self, ConnStatus};
use crate::net::connect_retry;

const FN_KEYCODE: i64 = 0x3F; // kVK_Function (Fn / Globe)
const CAPSLOCK_KEYCODE: i64 = 0x39; // kVK_CapsLock — acts as a TOGGLE in flagsChanged
const DOUBLE_TAP: Duration = Duration::from_millis(400);

// Raw FFI to re-enable the tap from inside the callback when the system disables it
// (TapDisabledByTimeout/ByUserInput). The core-graphics wrapper's enable() is called on
// the tap handle, but the handle cannot move into the callback (CFMachPort is !Send and
// CGEventTap::new wants a Send closure), so the CFMachPortRef is carried in an
// AtomicUsize and the call is made manually here. The callback runs on the main thread.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapEnable(tap: *mut std::ffi::c_void, enable: bool);
}

// First-run permission flow (macOS 10.15+): the preflight probes Input Monitoring
// WITHOUT prompting (true when granted). The request shows Apple's OFFICIAL permission
// dialog AND adds the app to System Settings > Privacy & Security > Input Monitoring
// automatically — the user does not have to find the app by hand.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

/// Accessibility permission: when called with kAXTrustedCheckOptionPrompt=true and the
/// permission is MISSING, Apple's official system dialog appears and the app is added
/// to the Accessibility list automatically; when ALREADY granted it returns true with
/// no dialog at all.
fn ax_trusted_with_prompt() -> bool {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        #[allow(non_upper_case_globals)]
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }
    unsafe {
        // Get rule: we do not own the system constant, so it is retained.
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let opts = CFDictionary::from_CFType_pairs(&[(
            key.as_CFType(),
            CFBoolean::true_value().as_CFType(),
        )]);
        AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef())
    }
}

/// Accessibility state WITHOUT any prompt — the wizard's "check" button must be
/// able to poll silently (ax_trusted_with_prompt would re-show the system dialog).
fn ax_is_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

/// Open the System Settings > Privacy & Security > Input Monitoring pane directly
/// (for the 'Open System Settings' buttons in the permission dialogs).
fn open_input_monitoring_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        .spawn();
}

/// Same, for the Accessibility pane (second wizard step).
fn open_accessibility_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

/// The enclosing .app bundle when the executable runs from one
/// (…/Name.app/Contents/MacOS/binary). None for bare `cargo run` binaries.
fn app_bundle_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().map_or(false, |e| e == "app"))
        .map(Into::into)
}

/// Restart the app so CGEventTap re-evaluates a just-granted permission (macOS
/// checks it at process start). 'sleep 1' lets THIS process exit first so the
/// single-instance lock port (main.rs) is free for the new instance; "$0" carries
/// the bundle path into `open` without shell-quoting pitfalls. Outside an .app
/// bundle (cargo run) `open -n` cannot target us, so the user restarts by hand.
fn relaunch_and_exit(mtm: MainThreadMarker) -> ! {
    if let Some(bundle) = app_bundle_path() {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg("sleep 1; open -n \"$0\"")
            .arg(&bundle)
            .spawn();
    } else {
        menubar::show_alert(
            mtm,
            "Permission granted",
            "The app quits now — start keyboard-it again by hand to finish.\n\
             (It is running outside an .app bundle, so it cannot restart itself.)",
        );
    }
    NSApplication::sharedApplication(mtm).terminate(None);
    // terminate: exits the process itself; this line is unreachable belt-and-braces.
    std::process::exit(0);
}

/// One guided permission step: intro alert -> official system prompt -> a
/// check-and-restart loop. Returns false when the user postponed ("Later").
/// On a successful check it relaunches the app and never returns.
fn wizard_step(
    mtm: MainThreadMarker,
    name: &str,
    intro_title: &str,
    intro_text: &str,
    fire_prompt: &dyn Fn(),
    granted: &dyn Fn() -> bool,
    open_pane: &dyn Fn(),
) -> bool {
    if menubar::show_choice_alert(mtm, intro_title, intro_text, &["Continue", "Later"]) != 0 {
        return false;
    }
    fire_prompt();
    loop {
        let text = format!(
            "Switch keyboard-it ON in the {name} prompt (or under System Settings \u{2192} \
             Privacy & Security \u{2192} {name}), then come back here.\n\n\
             macOS applies the permission only after a restart, so keyboard-it restarts \
             itself once you confirm."
        );
        match menubar::show_choice_alert(
            mtm,
            &format!("{name} — waiting for the permission"),
            &text,
            &["I granted it — check & restart", "Open System Settings", "Later"],
        ) {
            0 => {
                if granted() {
                    relaunch_and_exit(mtm);
                }
                menubar::show_alert(
                    mtm,
                    &format!("{name} is not granted yet"),
                    "macOS does not report the permission yet. Use 'Open System Settings', \
                     switch keyboard-it ON there, then try again.",
                );
            }
            1 => open_pane(),
            _ => return false,
        }
    }
}

/// Guided first-run permission chain (replaces bare official prompts): Input
/// Monitoring first; Accessibility follows on the next launch (each grant needs a
/// relaunch anyway, and one system prompt at a time is less confusing). Returns
/// true only when everything is ALREADY granted (no dialog shown); false when the
/// user postponed — the caller then skips the duplicate tap-failure alert.
fn permission_wizard(mtm: MainThreadMarker) -> bool {
    if !unsafe { CGPreflightListenEventAccess() } {
        return wizard_step(
            mtm,
            "Input Monitoring",
            "Welcome to keyboard-it",
            "keyboard-it forwards your keyboard and mouse to a Windows PC. macOS asks you \
             to grant two permissions first:\n\n\
             \u{2022} Input Monitoring — lets the app see keystrokes so it can forward them.\n\
             \u{2022} Accessibility — lets the app keep those keystrokes from also typing \
             on the Mac.\n\n\
             Continue brings up the system prompt for Input Monitoring (Accessibility \
             follows after a restart).",
            &|| {
                let _ = unsafe { CGRequestListenEventAccess() };
            },
            &|| unsafe { CGPreflightListenEventAccess() },
            &open_input_monitoring_settings,
        );
    }
    if !ax_is_trusted() {
        return wizard_step(
            mtm,
            "Accessibility",
            "One more permission",
            "Input Monitoring is granted. The last permission is Accessibility, which lets \
             keyboard-it hold keystrokes back from the Mac while you type on Windows.\n\n\
             Continue brings up the system prompt.",
            &|| {
                let _ = ax_trusted_with_prompt();
            },
            &ax_is_trusted,
            &open_accessibility_settings,
        );
    }
    true
}

/// Channel capacity: while disconnected, callback events above this limit are DROPPED
/// (try_send). The send thread also drains the queue once the connection is up, so a
/// reconnect does not replay a flood of stale keys.
const EVENT_QUEUE_CAP: usize = 128;

/// Set by the settings window's Save (settings.rs). The connection thread polls it
/// about once a second and drops the current connection so the just-saved
/// address/key applies immediately instead of when the old connection happens to die.
pub static CONFIG_DIRTY: AtomicBool = AtomicBool::new(false);

/// CGEventFlags mask that determines the down/up state of a modifier keycode.
fn modifier_mask(kc: i64) -> Option<CGEventFlags> {
    let m = match kc {
        0x37 | 0x36 => CGEventFlags::CGEventFlagCommand,
        0x38 | 0x3C => CGEventFlags::CGEventFlagShift,
        0x3A | 0x3D => CGEventFlags::CGEventFlagAlternate,
        0x3B | 0x3E => CGEventFlags::CGEventFlagControl,
        0x39 => CGEventFlags::CGEventFlagAlphaShift,
        _ => return None,
    };
    Some(m)
}

struct State {
    active: bool,
    fn_down: bool,
    last_fn_press: Option<Instant>,
    held: HashSet<u16>, // HID usages sent Down to Windows while ACTIVE
}

/// Detach/attach the Mac cursor from the physical mouse. With `captured=true` the cursor
/// FREEZES (WindowServer stops moving it for physical mouse input) but the CGEventTap
/// still sees deltas — needed because dropping the event does not stop the cursor.
/// Edge clamping is lifted too, so deltas keep flowing even at screen edges. Re-attached
/// when INACTIVE.
#[cfg(target_os = "macos")]
fn set_mouse_captured(captured: bool) {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        // boolean_t (= int): connected=1 normal (attached), 0 = detached (cursor freezes).
        fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
    }
    unsafe {
        let _ = CGAssociateMouseAndMouseCursorPosition(if captured { 0 } else { 1 });
    }
}

/// Is sender-side first-run setup complete? (Key in config or env AND peer_host set.)
fn config_ready(cfg: &protocol::config::Config) -> bool {
    protocol::secure::psk_from_config_or_env(cfg).is_ok() && !cfg.peer_host.is_empty()
}

pub fn run(cfg: protocol::config::Config) -> io::Result<()> {
    // Menu bar + event tap FIRST (main thread), connection in the BACKGROUND: the app
    // must not die silently when Windows is off or the config is empty.
    let mtm = MainThreadMarker::new()
        .expect("run() must be called on the main thread (AppKit requires it)");
    let initial_conn =
        if config_ready(&cfg) { ConnStatus::Connecting } else { ConnStatus::ConfigNeeded };
    let menu_bar = menubar::setup(mtm, false, initial_conn);

    // State flags: the tap callback and the connection thread (must be Send) write here;
    // a main-thread timer reads them and updates the menu bar title (objc2 objects are !Send).
    let active_flag = Arc::new(AtomicBool::new(false));
    let conn_status = Arc::new(AtomicU8::new(initial_conn as u8));
    let permission_needed = Arc::new(AtomicBool::new(false));
    menubar::install_status_updater(
        mtm,
        menu_bar.status_item.clone(),
        active_flag.clone(),
        conn_status.clone(),
        permission_needed.clone(),
    );
    // The settings window's status line mirrors the same connection state.
    crate::settings::set_conn_status_source(conn_status.clone());
    let flag_cb = active_flag.clone();

    println!("State: INACTIVE. Double-tap Fn to toggle.");
    println!("(Permissions: Input Monitoring + Accessibility. Prerequisite: fn key set to 'Do Nothing'.)");
    println!("(Quit: Ctrl-C — or  > Force Quit with the mouse if stuck.)");

    // Keep the callback light: push events into a BOUNDED channel (try_send — drop when
    // full); a separate thread writes them framed to TCP. An unbounded queue let stale
    // events pile up during an outage and flood the peer on reconnect.
    let (tx, rx) = mpsc::sync_channel::<InputEvent>(EVENT_QUEUE_CAP);
    let conn_bg = conn_status.clone();
    thread::spawn(move || loop {
        // Clear the dirty flag BEFORE reading the config: if a Save lands after this
        // point the flag stays set and the send loop below drops the connection within
        // a second — no Save is ever missed (worst case one redundant reconnect).
        CONFIG_DIRTY.store(false, Ordering::Relaxed);
        // Re-read the config on EVERY attempt: an address/key changed via 'Settings...'
        // takes effect on the next attempt without a restart. Broken/missing file =
        // 'Setup needed'.
        let cfg = protocol::config::Config::load().ok().flatten().unwrap_or_default();
        let psk = match protocol::secure::psk_from_config_or_env(&cfg) {
            Ok(p) if !cfg.peer_host.is_empty() => p,
            _ => {
                conn_bg.store(ConnStatus::ConfigNeeded as u8, Ordering::Relaxed);
                for _ in rx.try_iter() {} // drop events while disconnected
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let addr = cfg.peer_addr();
        conn_bg.store(ConnStatus::Connecting as u8, Ordering::Relaxed);
        let mut stream = match connect_retry(&addr) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("connect failed ({addr}): {e} — will retry.");
                conn_bg.store(ConnStatus::Disconnected as u8, Ordering::Relaxed);
                for _ in rx.try_iter() {}
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let mut transport = match protocol::secure::handshake_initiator(&mut stream, &psk) {
            Ok(t) => t,
            Err(e) => {
                // secure.rs reports a wrong key as the peer rejecting the handshake and
                // asks whether the pairing key matches on both sides.
                eprintln!("handshake failed: {e}");
                conn_bg.store(ConnStatus::HandshakeFailed as u8, Ordering::Relaxed);
                for _ in rx.try_iter() {}
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        println!("connected (encrypted, Noise NNpsk0): {addr}");
        conn_bg.store(ConnStatus::Connected as u8, Ordering::Relaxed);
        // Anything queued during the outage is stale — drain before sending.
        for _ in rx.try_iter() {}
        // Send loop — until the connection drops or the settings change.
        loop {
            // 'Save' in the settings window flips CONFIG_DIRTY. It must take effect
            // even when no keys are flowing, so recv() below has a ~1 s timeout
            // instead of blocking forever, and the flag is checked every pass.
            if CONFIG_DIRTY.swap(false, Ordering::Relaxed) {
                println!("settings changed — reconnecting.");
                conn_bg.store(ConnStatus::Connecting as u8, Ordering::Relaxed);
                break; // outer loop re-reads the config and reconnects
            }
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(ev) => {
                    if protocol::secure::send_event(&mut transport, &mut stream, &ev).is_err() {
                        eprintln!("connection lost — reconnecting...");
                        conn_bg.store(ConnStatus::Disconnected as u8, Ordering::Relaxed);
                        break; // outer loop reconnects
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {} // idle tick: re-check the flag
                Err(mpsc::RecvTimeoutError::Disconnected) => return, // main thread gone
            }
        }
    });

    // Guided permission flow FIRST (before the setup alert): on a fresh install the
    // wizard walks through Input Monitoring and — after a self-relaunch — Accessibility,
    // so the config alert below only appears once the relaunch cycle is over. false =
    // the user postponed; remembered so the tap-failure path does not repeat the same
    // explanation in a second alert.
    let permissions_ok = permission_wizard(mtm);

    // First launch with an empty config: show VISIBLE instructions (stderr is invisible
    // in an LSUIElement .app) and take the user straight to the settings window.
    if initial_conn == ConnStatus::ConfigNeeded {
        let open = menubar::show_setup_alert(
            mtm,
            "keyboard-it — first-run setup",
            "Two details are needed before the first connection:\n\n\
             \u{2022} Pairing key — click Generate and enter the SAME key on the Windows \
             side (or paste the key already set there).\n\
             \u{2022} Your Windows PC — pick it from the discovered list, or type its \
             address by hand.\n\n\
             Once saved, the app connects on its own (no restart needed).",
        );
        if open {
            crate::settings::open(mtm);
        }
    }

    let state = RefCell::new(State {
        active: false,
        fn_down: false,
        last_fn_press: None,
        held: HashSet::new(),
    });

    // The tap's mach port: when the callback sees TapDisabled* it re-enables the tap via
    // CGEventTapEnable — without this the toggle dies silently and the app becomes
    // unusable. Filled in AFTER the tap is created (0 = not yet).
    let tap_port = Arc::new(AtomicUsize::new(0));
    let tap_port_cb = tap_port.clone();

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        // ACTIVE tap: returning Drop from the callback swallows the key (needs Accessibility).
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            // mouse: movement (plain + dragging with a button held)
            CGEventType::MouseMoved,
            CGEventType::LeftMouseDragged,
            CGEventType::RightMouseDragged,
            CGEventType::OtherMouseDragged,
            // mouse: buttons
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            // mouse: scroll
            CGEventType::ScrollWheel,
        ],
        move |_proxy, event_type, event: &CGEvent| -> CallbackResult {
            // If the system disabled the tap (timeout/user input), re-enable at once:
            // otherwise the double-Fn toggle dies silently and the app is unusable.
            if matches!(
                event_type,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
            ) {
                let port = tap_port_cb.load(Ordering::Relaxed);
                if port != 0 {
                    unsafe { CGEventTapEnable(port as *mut std::ffi::c_void, true) };
                    eprintln!("warning: event tap had been disabled ({event_type:?}) — re-enabled.");
                } else {
                    eprintln!("warning: event tap disabled ({event_type:?}).");
                }
                return CallbackResult::Keep;
            }

            let kc = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let mut st = state.borrow_mut();

            // --- Fn key: double-tap toggle detection (always, regardless of state) ---
            if kc == FN_KEYCODE {
                let now_down = event.get_flags().contains(CGEventFlags::CGEventFlagSecondaryFn);
                if now_down && !st.fn_down {
                    // rising edge = one "tap"
                    let is_double = matches!(st.last_fn_press, Some(t) if t.elapsed() <= DOUBLE_TAP);
                    if is_double {
                        st.last_fn_press = None;
                        st.active = !st.active;
                        if st.active {
                            set_mouse_captured(true); // freeze the Mac cursor
                            println!(">>> ACTIVE — keyboard+mouse go to Windows (suppressed on the Mac).");
                        } else {
                            set_mouse_captured(false); // re-attach the Mac cursor
                            // Returning to INACTIVE: release keys still held down on Windows.
                            let held: Vec<u16> = st.held.drain().collect();
                            for hid in held {
                                let _ = tx.try_send(InputEvent::Key(KeyEvent {
                                    msg: MsgType::Up,
                                    hid_usage: hid,
                                    modifiers: 0,
                                }));
                            }
                            println!("<<< INACTIVE — keyboard+mouse back on the Mac.");
                        }
                        // Update the state flag; the main-thread timer reflects it in the menu bar.
                        flag_cb.store(st.active, Ordering::Relaxed);
                    } else {
                        st.last_fn_press = Some(Instant::now());
                    }
                }
                st.fn_down = now_down;
                // Fn itself never goes to Windows (no HID usage). Consume while ACTIVE,
                // pass through while INACTIVE.
                return if st.active { CallbackResult::Drop } else { CallbackResult::Keep };
            }

            // --- INACTIVE: let the Mac work normally; do not send, do not suppress ---
            if !st.active {
                return CallbackResult::Keep;
            }

            // --- ACTIVE: translate + send + suppress ---
            match event_type {
                CGEventType::KeyDown => {
                    let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT);
                    if repeat == 0 {
                        if let Some(hid) = mac_keycode_to_hid(kc) {
                            st.held.insert(hid);
                            let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }));
                        }
                    }
                }
                CGEventType::KeyUp => {
                    if let Some(hid) = mac_keycode_to_hid(kc) {
                        st.held.remove(&hid);
                        let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }));
                    }
                }
                CGEventType::FlagsChanged => {
                    if kc == CAPSLOCK_KEYCODE {
                        // CapsLock: in flagsChanged the AlphaShift flag is a TOGGLE, not a
                        // physical down/up — mapping the flag to Down/Up desynchronized
                        // the two sides. Send one Down+Up pair per change instead:
                        // Windows toggles exactly once. Not tracked in `held` (released
                        // immediately).
                        if let Some(hid) = mac_keycode_to_hid(kc) {
                            let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }));
                            let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }));
                        }
                    } else if let (Some(hid), Some(mask)) = (mac_keycode_to_hid(kc), modifier_mask(kc)) {
                        let down = event.get_flags().contains(mask);
                        if down {
                            st.held.insert(hid);
                        } else {
                            st.held.remove(&hid);
                        }
                        let msg = if down { MsgType::Down } else { MsgType::Up };
                        let _ = tx.try_send(InputEvent::Key(KeyEvent { msg, hid_usage: hid, modifiers: 0 }));
                    }
                }

                // --- mouse: RELATIVE movement (deltas are only meaningful for move/drag) ---
                CGEventType::MouseMoved
                | CGEventType::LeftMouseDragged
                | CGEventType::RightMouseDragged
                | CGEventType::OtherMouseDragged => {
                    let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X);
                    let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y);
                    let dx = dx.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                    let dy = dy.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                    if dx != 0 || dy != 0 {
                        let _ = tx.try_send(InputEvent::MouseMove { dx, dy });
                    }
                }

                // --- mouse: left/right buttons (dedicated event types) ---
                CGEventType::LeftMouseDown => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::LEFT, down: true });
                }
                CGEventType::LeftMouseUp => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::LEFT, down: false });
                }
                CGEventType::RightMouseDown => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::RIGHT, down: true });
                }
                CGEventType::RightMouseUp => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::RIGHT, down: false });
                }

                // --- mouse: other buttons (middle = number 2; extras skipped for now) ---
                CGEventType::OtherMouseDown | CGEventType::OtherMouseUp => {
                    let num = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
                    let down = matches!(event_type, CGEventType::OtherMouseDown);
                    if num == 2 {
                        let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::MIDDLE, down });
                    }
                }

                // --- mouse: scroll. Axis1=vertical, Axis2=horizontal (integer ticks). ---
                CGEventType::ScrollWheel => {
                    let v = event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
                    let h = event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2);
                    let dy = v.clamp(i8::MIN as i64, i8::MAX as i64) as i8;
                    let dx = h.clamp(i8::MIN as i64, i8::MAX as i64) as i8;
                    if dx != 0 || dy != 0 {
                        let _ = tx.try_send(InputEvent::Scroll { dx, dy });
                    }
                }

                _ => {}
            }
            CallbackResult::Drop // while ACTIVE, suppress all keyboard+mouse events on the Mac
        },
    );

    // If the tap cannot be created (typical cause: missing Input Monitoring or
    // Accessibility permission), do NOT exit: the .app is LSUIElement, so stderr is
    // invisible and a silent exit looks like the app never launched. Show a visible
    // dialog instead; the app stays open with a 'Permission needed' menu bar title
    // (Settings/Quit keep working).
    let _tap = match tap {
        Ok(tap) => match tap.mach_port().create_runloop_source(0) {
            Ok(source) => {
                unsafe {
                    CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
                }
                // Hand the port to the callback (TapDisabled* recovery).
                tap_port.store(tap.mach_port().as_concrete_TypeRef() as usize, Ordering::Relaxed);
                tap.enable();
                println!("ready. Double-tap Fn → ACTIVE; double-tap again → INACTIVE. (Quit via the menu bar.)");
                Some(tap)
            }
            Err(_) => {
                permission_needed.store(true, Ordering::Relaxed);
                menubar::show_alert(
                    mtm,
                    "keyboard-it — error",
                    "Keyboard capture could not start (failed to create the run loop source).\n\
                     Try quitting and reopening the app.",
                );
                None
            }
        },
        Err(_) => {
            permission_needed.store(true, Ordering::Relaxed);
            // When the wizard was postponed ("Later") this failure is expected and
            // already explained — a second alert would just nag. Only surface the
            // unexpected case: wizard says both granted, yet the tap still failed.
            if permissions_ok {
                let open = menubar::show_setup_alert(
                    mtm,
                    "keyboard-it — permission needed",
                    "Keyboard capture could not start — a permission is missing.\n\n\
                     If you granted the permission in the system prompt that appeared:\n\
                     macOS requires the app to be quit and REOPENED for it to take effect.\n\n\
                     If no prompt appeared: enable keyboard-it under System Settings \u{2192}\n\
                     Privacy & Security \u{2192} Input Monitoring (and Accessibility),\n\
                     then restart the app.\n\n\
                     The app will stay in the menu bar as 'Permission needed'.",
                );
                if open {
                    open_input_monitoring_settings();
                }
            }
            None
        }
    };

    // Run AppKit AFTER the tap source is added to the main run loop. app.run() drives
    // the same main-thread CFRunLoop; the source is in kCFRunLoopCommonModes, so the tap
    // also fires under NSApp.
    let app = NSApplication::sharedApplication(mtm);
    app.run();

    drop(menu_bar); // keep the handles alive up to this point
    Ok(())
}
