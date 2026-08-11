//! The macOS permissions keyboard-it needs, and the one window that walks the
//! user through all of them.
//!
//! The old flow asked for ONE permission per launch: Input Monitoring, then a
//! self-relaunch, then Accessibility on the next launch, and Local Network never
//! — only a sentence about it inside another dialog. From the user's side that
//! reads as "it keeps sending me to Input Monitoring", with no way to see how
//! many permissions there are or which ones are done.
//!
//! So: one window, a fixed order (Input Monitoring, Accessibility, Local
//! Network), a live status per row, the next system prompt fired automatically
//! as each one lands, and a SINGLE restart at the end — macOS only applies these
//! at process start, and the old flow paid that cost once per permission.
//!
//! Non-modal by design. The window is driven by its own 1 s NSTimer, so it needs
//! the run loop that NSAlert's nested modal loop used to stand in for; and the
//! system prompts it raises belong to other processes, which a modal would
//! obscure rather than sequence.

#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSFont, NSLineBreakMode,
    NSPasteboard, NSPasteboardTypeString, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSTimer,
};

use crate::discovery;

// ---------------------------------------------------------------------------
// The system calls behind each permission
// ---------------------------------------------------------------------------

// The preflight probes Input Monitoring WITHOUT prompting (true when granted).
// The request shows Apple's OFFICIAL permission dialog AND adds the app to
// System Settings > Privacy & Security > Input Monitoring automatically — the
// user does not have to find the app by hand.
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
    use core_foundation::base::TCFType;
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

/// Accessibility state WITHOUT any prompt — the window polls once a second, and
/// ax_trusted_with_prompt would re-show the system dialog on every tick.
fn ax_is_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

/// The two permissions the CGEventTap actually depends on. Synchronous, silent
/// and cheap — this is what capture::run gates on. Local Network is deliberately
/// absent: it has no queryable API, and the tap does not need it.
pub fn tap_permissions_granted() -> bool {
    (unsafe { CGPreflightListenEventAccess() }) && ax_is_trusted()
}

/// Set when macOS reports both permissions as granted and the tap STILL failed.
///
/// This is the update case: macOS ties a permission to the exact binary it was
/// granted to (for an unsigned app, its code hash), and every keyboard-it update
/// is a different binary — so the row survives the update while the grant behind
/// it does not. The switch reads ON, the app is denied, and nothing on screen
/// connects the two. The window says so instead of reporting a cheerful "Granted".
static STALE_GRANT: AtomicBool = AtomicBool::new(false);

/// Called by capture::run when the tap failed despite both checks passing.
pub fn note_stale_grant() {
    STALE_GRANT.store(true, Ordering::Relaxed);
}

/// How long a fired system prompt gets to change something before the row starts
/// offering the recovery as well.
///
/// Long on purpose. The Input Monitoring and Accessibility prompts do not grant
/// anything by themselves — they only offer to open System Settings, where the
/// user still has to find the row and authenticate — so a perfectly healthy grant
/// easily takes half a minute. Anything shorter accuses macOS of ignoring a
/// prompt that is still sitting on screen.
///
/// Input Monitoring and Accessibility live in the SYSTEM TCC database, and macOS
/// keeps exactly one answer per bundle identifier there. Once that answer exists
/// — a denial, or an approval bound to an earlier build's cdhash — the request
/// APIs return silently and no dialog is ever shown again. An unsigned app is
/// always in this position eventually: its designated requirement is its code
/// hash, so every rebuild invalidates the approval while leaving the row behind.
///
/// The app cannot repair that itself (the system database needs root), so the
/// only honest move is to stop showing a "Grant…" button that does nothing and
/// say what actually clears it.
const STUCK_GRACE: Duration = Duration::from_secs(30);

/// This app's bundle identifier, for the tccutil command the window offers.
fn bundle_id() -> String {
    objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "com.keyboard-it.keyboard-it".to_string())
}

/// Put `text` on the general pasteboard.
fn copy_to_clipboard(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    unsafe {
        pb.clearContents();
        let _ = pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString);
    }
}

/// The enclosing .app bundle when the executable runs from one
/// (…/Name.app/Contents/MacOS/binary). None for bare `cargo run` binaries.
fn app_bundle_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().map_or(false, |e| e == "app"))
        .map(Into::into)
}

/// Can the app restart itself? Decides the last button's title UP FRONT, so the
/// user is not told "Restart" and then handed a "start it again by hand" alert.
pub fn can_relaunch() -> bool {
    app_bundle_path().is_some()
}

/// Restart the app so CGEventTap re-evaluates a just-granted permission (macOS
/// checks it at process start). 'sleep 1' lets THIS process exit first so the
/// single-instance lock port (main.rs) is free for the new instance; "$0" carries
/// the bundle path into `open` without shell-quoting pitfalls. Outside an .app
/// bundle (cargo run) `open -n` cannot target us, so the user restarts by hand.
pub fn relaunch_and_exit(mtm: MainThreadMarker) -> ! {
    if let Some(bundle) = app_bundle_path() {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg("sleep 1; open -n \"$0\"")
            .arg(&bundle)
            .spawn();
    }
    NSApplication::sharedApplication(mtm).terminate(None);
    // terminate: exits the process itself; this line is unreachable belt-and-braces.
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PermState {
    /// Local Network only: its turn has not come up, so nothing has been asked
    /// yet. Distinct from Checking because a row that says "Checking…" while
    /// nothing is running is the same kind of lie this window exists to remove.
    NotChecked,
    /// Asked, no answer yet. Only ever Local Network, while the browse runs.
    Checking,
    /// Local Network only: macOS publishes no API for reading this permission, and
    /// the indirect evidence came back empty. That is NOT a denial — an allowed
    /// app on a network with nothing to find looks exactly the same — so the row
    /// says it cannot tell rather than inventing a verdict.
    Unknowable,
    Granted,
    /// Input Monitoring / Accessibility: macOS says no. Local Network: nothing
    /// answered within DISCOVERY_GRACE — a HINT, not a fact, because a network
    /// with no receiver on it looks exactly like a denied permission. Never used
    /// to block the user.
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    InputMonitoring,
    Accessibility,
    LocalNetwork,
}

/// The order the user is walked through, and the order the rows are drawn in.
/// Input Monitoring first because without it there is nothing to forward;
/// Accessibility second because it only matters once keys are being seen; Local
/// Network last because its prompt is raised by network traffic rather than by
/// an API call, and stacking it under the other two makes all three arrive at once.
pub const ORDER: [PermissionKind; 3] = [
    PermissionKind::InputMonitoring,
    PermissionKind::Accessibility,
    PermissionKind::LocalNetwork,
];

impl PermissionKind {
    fn title(self) -> &'static str {
        match self {
            Self::InputMonitoring => "Input Monitoring",
            Self::Accessibility => "Accessibility",
            Self::LocalNetwork => "Local Network",
        }
    }

    fn why(self) -> &'static str {
        match self {
            Self::InputMonitoring => "Lets keyboard-it see your keystrokes, so it can forward them.",
            Self::Accessibility => "Lets it stop those keystrokes from also typing on the Mac.",
            Self::LocalNetwork => "Lets it find your PC here \u{2014} no IP address to type.",
        }
    }

    /// Silent — this runs on a 1 s timer, so nothing here may prompt.
    fn check(self) -> PermState {
        match self {
            Self::InputMonitoring => {
                if unsafe { CGPreflightListenEventAccess() } {
                    PermState::Granted
                } else {
                    PermState::Missing
                }
            }
            Self::Accessibility => {
                if ax_is_trusted() {
                    PermState::Granted
                } else {
                    PermState::Missing
                }
            }
            // No API exists for this one, so it is answered by evidence or not at
            // all. Two things are PROOF that local network access works: something
            // answered the mDNS browse, or there is a live TCP session with the PC
            // (which is the case for an already-paired user whose PC is simply not
            // advertising at this instant). Absence of both proves nothing.
            Self::LocalNetwork => {
                if discovery::ever_answered() || crate::settings::is_connected() {
                    PermState::Granted
                } else {
                    match discovery::elapsed() {
                        None => PermState::NotChecked,
                        Some(d) if d >= discovery::DISCOVERY_GRACE => PermState::Unknowable,
                        Some(_) => PermState::Checking,
                    }
                }
            }
        }
    }

    /// Fire the OFFICIAL system prompt. Returns immediately in all three cases
    /// (the IM/AX dialogs are drawn out-of-process, the browse is a thread), so
    /// this never blocks the run loop that the window's timer needs.
    fn request(self) {
        match self {
            Self::InputMonitoring => {
                let _ = unsafe { CGRequestListenEventAccess() };
            }
            Self::Accessibility => {
                let _ = ax_trusted_with_prompt();
            }
            // There is no "ask" call for Local Network — the first multicast IS
            // the ask, and macOS raises the prompt on our behalf.
            Self::LocalNetwork => discovery::start(),
        }
    }

    /// Open the matching System Settings pane (the manual fallback once the
    /// one-shot system prompt has been spent).
    fn open_pane(self) {
        let url = match self {
            Self::InputMonitoring => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
            }
            Self::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Self::LocalNetwork => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"
            }
        };
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
}

// ---------------------------------------------------------------------------
// Watching for a grant that lands while the app is already running
// ---------------------------------------------------------------------------

/// CGEventTap evaluates Input Monitoring and Accessibility once, at process
/// start, so granting them in System Settings afterwards changes nothing until a
/// relaunch — and the menu bar goes on saying "Permission needed" over a settings
/// pane the user can see is switched on. Poll instead, and the moment both are
/// actually granted, flip the state to "only a restart is left".
///
/// No dialog: the menu bar turns 🔄, its Permissions item retitles itself, and
/// the permissions window (whose own timer is running) arms its Restart button.
/// The old version put up an alert here, which meant interrupting whatever the
/// user had moved on to.
///
/// Installed only when the tap failed, so a healthy run pays nothing for it.
pub fn install_watcher(permission_needed: Arc<AtomicBool>, restart_needed: Arc<AtomicBool>) {
    let block = RcBlock::new(move |t: NonNull<NSTimer>| {
        if !tap_permissions_granted() {
            return;
        }
        unsafe { t.as_ref().invalidate() };
        // The state is no longer "a permission is missing" — it is "the process
        // predates the permission", which is a different thing to tell the user.
        permission_needed.store(false, Ordering::Relaxed);
        restart_needed.store(true, Ordering::Relaxed);
    });
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &block);
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

// Main-thread-only (it holds AppKit objects), so a thread_local is the natural
// owner — same as settings.rs.
thread_local! {
    static CONTROLLER: RefCell<Option<Retained<PermissionsController>>> =
        const { RefCell::new(None) };
}

/// Open (create on first use) the permissions window and bring it to the front.
pub fn open(mtm: MainThreadMarker) {
    CONTROLLER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(PermissionsController::create(mtm));
        }
        let c = slot.as_ref().unwrap();
        // Accessory app (no Dock icon): without an explicit activation the window
        // can appear behind whatever the user was working in.
        let app = NSApplication::sharedApplication(mtm);
        unsafe {
            let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
        }
        c.ivars().window.makeKeyAndOrderFront(None);
        c.refresh();
    });
}

/// One permission's row of controls.
struct Row {
    kind: PermissionKind,
    status: Retained<NSTextField>,
    button: Retained<NSButton>,
}

struct Ivars {
    window: Retained<NSWindow>,
    rows: [Row; 3],
    restart_button: Retained<NSButton>,
    /// Shown only while a row is stuck. The System Settings dance is fiddly and
    /// easy to get wrong; `sudo tccutil reset` is the one command that always
    /// clears the saved answer, and the app cannot run it itself (no root).
    copy_button: Retained<NSButton>,
    footer: Retained<NSTextField>,
    /// When each row's prompt was fired, so a prompt that changed nothing can be
    /// told apart from one the user simply has not answered yet.
    prompted_at: [Cell<Option<Instant>>; 3],
    /// Keeps the "Copied ✓" confirmation on screen past the next repaint.
    copied_until: Cell<Option<Instant>>,
    /// Each row's system prompt is fired AT MOST ONCE per process. macOS only
    /// shows it once anyway; this is what stops a 1 s timer from re-firing it,
    /// and it is also what makes the button switch to "Open System Settings".
    requested: [Cell<bool>; 3],
    /// Never two prompts within 2 s of each other, so a tccutil reset while the
    /// window is open cannot stack two system dialogs in one tick.
    last_prompt: Cell<Option<Instant>>,
    /// What the last repaint drew. Most ticks change nothing, and rewriting
    /// unchanged AppKit strings makes the window flicker under VoiceOver.
    /// `requested`, the stuck flags and the stale-grant flag are part of the key
    /// because they decide the button titles and the status text.
    shown: Cell<Option<([PermState; 3], usize, [bool; 3], [bool; 3], bool, bool)>>,
    /// Decided once at create(): outside an .app bundle there is nothing to
    /// relaunch, so the last button quits instead.
    can_relaunch: bool,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KbItPermissionsController"]
    #[ivars = Ivars]
    struct PermissionsController;

    unsafe impl NSObjectProtocol for PermissionsController {}

    impl PermissionsController {
        // Three thin selectors rather than one action that compares sender
        // pointers: the row index is known at wiring time, so there is nothing
        // to recover at click time.
        #[unsafe(method(row0:))]
        fn row0(&self, _sender: Option<&AnyObject>) {
            self.row_action(0);
        }

        #[unsafe(method(row1:))]
        fn row1(&self, _sender: Option<&AnyObject>) {
            self.row_action(1);
        }

        #[unsafe(method(row2:))]
        fn row2(&self, _sender: Option<&AnyObject>) {
            self.row_action(2);
        }

        #[unsafe(method(restart:))]
        fn restart(&self, _sender: Option<&AnyObject>) {
            relaunch_and_exit(self.mtm());
        }

        #[unsafe(method(copyReset:))]
        fn copyReset(&self, _sender: Option<&AnyObject>) {
            copy_to_clipboard(&format!("sudo tccutil reset All {}", bundle_id()));
            let iv = self.ivars();
            // Held for a few seconds: without it the 1 s repaint would wipe the
            // confirmation before the user's eye got back to the button.
            iv.copied_until.set(Some(Instant::now() + Duration::from_secs(4)));
            iv.shown.set(None);
            self.refresh();
        }

        // 1 s heartbeat while the window is visible: re-check all three, fire the
        // next prompt if the current row has not been asked yet, repaint.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            let iv = self.ivars();
            if !iv.window.isVisible() {
                return;
            }
            self.refresh();
        }
    }
);

// Layout constants. AppKit's origin is bottom-left, so every `y` below is a
// distance UP from the bottom of the content view.
const W: f64 = 620.0; // content width
const PAD: f64 = 20.0; // left/right margin
const BTN_W: f64 = 164.0; // per-row action button
const BTN_X: f64 = W - PAD - BTN_W; // right-aligned
const TEXT_W: f64 = BTN_X - PAD - 16.0; // labels stop short of the buttons
const ROW_PITCH: f64 = 80.0;
const ROW0_Y: f64 = 326.0; // baseline of the first row's title

impl PermissionsController {
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        // No resize and no minimise: the layout is fixed and the window is a
        // step in a flow, not something to park in the Dock.
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        // Sized from the content, not guessed: three 80 pt rows, a header, a
        // footer that has to hold three wrapped lines, and a button strip.
        let content = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(W, 430.0));
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
        window.setTitle(ns_string!("keyboard-it — Permissions"));
        window.center();

        let label = |text: &NSString, x: f64, y: f64, w: f64, h: f64| -> Retained<NSTextField> {
            let l = NSTextField::labelWithString(text, mtm);
            l.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)));
            l
        };
        let small = |l: &NSTextField| {
            l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            l.setTextColor(Some(&NSColor::secondaryLabelColor()));
        };

        let header = label(
            ns_string!("keyboard-it needs three permissions from macOS, in this order:"),
            PAD,
            388.0,
            W - 2.0 * PAD,
            17.0,
        );

        let content_view = window.contentView().expect("titled window has a content view");
        content_view.addSubview(&header);

        // Rows top to bottom in AppKit's bottom-left coordinates: row i sits
        // ROW_PITCH below row i-1.
        let mut rows: Vec<Row> = Vec::with_capacity(3);
        for (i, kind) in ORDER.iter().enumerate() {
            let y = ROW0_Y - ROW_PITCH * i as f64;

            let title = label(&NSString::from_str(kind.title()), PAD, y, 240.0, 17.0);
            let why = label(&NSString::from_str(kind.why()), PAD, y - 22.0, TEXT_W, 15.0);
            small(&why);
            let status = label(ns_string!(""), PAD, y - 42.0, TEXT_W, 15.0);
            small(&status);

            let button = unsafe {
                NSButton::buttonWithTitle_target_action(ns_string!("Grant\u{2026}"), None, None, mtm)
            };
            button.setFrame(NSRect::new(
                NSPoint::new(BTN_X, y - 12.0),
                NSSize::new(BTN_W, 32.0),
            ));

            content_view.addSubview(&title);
            content_view.addSubview(&why);
            content_view.addSubview(&status);
            content_view.addSubview(&button);

            // No separator under the last row — the footer already divides it
            // from the Restart button.
            if i < 2 {
                let sep = NSBox::initWithFrame(
                    NSBox::alloc(mtm),
                    NSRect::new(NSPoint::new(PAD, y - 56.0), NSSize::new(W - 2.0 * PAD, 1.0)),
                );
                sep.setBoxType(NSBoxType::Separator);
                content_view.addSubview(&sep);
            }

            rows.push(Row { kind: *kind, status, button });
        }
        let rows: [Row; 3] = rows.try_into().unwrap_or_else(|_| unreachable!("ORDER has 3"));

        // The footer carries the recovery instructions, which are a paragraph and
        // not a line: labelWithString clips by default, so wrapping is explicit.
        let footer = label(ns_string!(""), PAD, 58.0, W - 2.0 * PAD, 52.0);
        small(&footer);
        footer.setMaximumNumberOfLines(3);
        footer.setPreferredMaxLayoutWidth(W - 2.0 * PAD);
        footer.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        content_view.addSubview(&footer);

        let can_relaunch_now = can_relaunch();
        let restart_title = if can_relaunch_now {
            ns_string!("Restart keyboard-it")
        } else {
            ns_string!("Quit keyboard-it")
        };
        let restart_button =
            unsafe { NSButton::buttonWithTitle_target_action(restart_title, None, None, mtm) };
        restart_button.setFrame(NSRect::new(
            NSPoint::new(W - PAD - 170.0, 16.0),
            NSSize::new(170.0, 32.0),
        ));
        content_view.addSubview(&restart_button);

        let copy_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Copy reset command"),
                None,
                None,
                mtm,
            )
        };
        copy_button.setFrame(NSRect::new(NSPoint::new(PAD, 16.0), NSSize::new(264.0, 32.0)));
        copy_button.setHidden(true);
        content_view.addSubview(&copy_button);

        let this = Self::alloc(mtm).set_ivars(Ivars {
            window,
            rows,
            restart_button: restart_button.clone(),
            copy_button: copy_button.clone(),
            footer,
            requested: [Cell::new(false), Cell::new(false), Cell::new(false)],
            prompted_at: [Cell::new(None), Cell::new(None), Cell::new(None)],
            copied_until: Cell::new(None),
            last_prompt: Cell::new(None),
            shown: Cell::new(None),
            can_relaunch: can_relaunch_now,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Targets are wired after init: the controls must exist before the
        // controller (they live in its ivars), so they start target-less.
        let wire = |control: &NSButton, action: Sel| unsafe {
            control.setTarget(Some(&*this));
            control.setAction(Some(action));
        };
        wire(&this.ivars().rows[0].button, sel!(row0:));
        wire(&this.ivars().rows[1].button, sel!(row1:));
        wire(&this.ivars().rows[2].button, sel!(row2:));
        wire(&restart_button, sel!(restart:));
        wire(&copy_button, sel!(copyReset:));

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

    /// Re-check everything, advance the flow, redraw what changed. The single
    /// entry point: the timer calls it, every button calls it, open() calls it.
    fn refresh(&self) {
        let states = [ORDER[0].check(), ORDER[1].check(), ORDER[2].check()];
        // "current" = the first row in the fixed order that is still actionable.
        // Rows above it are done; rows below it are dimmed. There is exactly one
        // thing on screen to act on at any moment — that is the whole point.
        // Unknowable counts as settled: the browse has been started (which is what
        // raises the system prompt), and no amount of further waiting can turn a
        // missing API into an answer, so the flow must not park on it forever.
        let current = states
            .iter()
            .position(|s| !matches!(s, PermState::Granted | PermState::Unknowable))
            .unwrap_or(ORDER.len());
        self.maybe_auto_prompt(current);
        self.repaint(&states, current);
    }

    /// Fire the current row's system prompt, at most once, and never two in the
    /// same beat. This is the auto-advance: granting Input Monitoring flips its
    /// check() within a second, `current` moves to 1, and the NEXT tick raises
    /// the Accessibility prompt — without the user going back to a dialog.
    fn maybe_auto_prompt(&self, current: usize) {
        let iv = self.ivars();
        if current >= ORDER.len() || iv.requested[current].get() {
            return;
        }
        if iv.last_prompt.get().is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        // Don't ambush someone who has moved on to another app: a system prompt
        // stealing focus out of nowhere is worse than one they walked into.
        if !iv.window.isKeyWindow() {
            return;
        }
        iv.requested[current].set(true);
        iv.last_prompt.set(Some(Instant::now()));
        iv.prompted_at[current].set(Some(Instant::now()));
        ORDER[current].request();
    }

    /// A row whose prompt was fired and then changed nothing: macOS already has
    /// an answer saved for this bundle identifier, so it will never ask again and
    /// re-firing the request is a no-op. Local Network is exempt — its "prompt"
    /// is network traffic, and silence there means an empty network just as often
    /// as a refusal.
    fn stuck(&self, i: usize, state: PermState) -> bool {
        ORDER[i] != PermissionKind::LocalNetwork
            && state == PermState::Missing
            && self.ivars().prompted_at[i]
                .get()
                .is_some_and(|t| t.elapsed() >= STUCK_GRACE)
    }

    /// A row's button: the official prompt while it still has one to give, the
    /// System Settings pane afterwards.
    fn row_action(&self, i: usize) {
        let iv = self.ivars();
        // A stale row is already "granted" as far as macOS is concerned, so
        // re-firing the prompt would do nothing at all. The pane is the only
        // place the entry can be removed and re-added.
        let stale = STALE_GRANT.load(Ordering::Relaxed)
            && ORDER[i] != PermissionKind::LocalNetwork
            && ORDER[i].check() == PermState::Granted;
        if stale {
            ORDER[i].open_pane();
        } else if !iv.requested[i].get() {
            iv.requested[i].set(true);
            iv.last_prompt.set(Some(Instant::now()));
            iv.prompted_at[i].set(Some(Instant::now()));
            ORDER[i].request();
        } else {
            ORDER[i].open_pane();
        }
        self.refresh();
    }

    fn repaint(&self, states: &[PermState; 3], current: usize) {
        let iv = self.ivars();
        // requested[] is part of what the buttons show, so it belongs in the
        // change key — otherwise the first click would not retitle its button.
        let stale = STALE_GRANT.load(Ordering::Relaxed);
        let stuck = [
            self.stuck(0, states[0]),
            self.stuck(1, states[1]),
            self.stuck(2, states[2]),
        ];
        let any_stuck = stuck.iter().any(|s| *s) || stale;
        // `copied` expires on a clock rather than on a state change, so it has to
        // be in the key or the confirmation would never revert.
        let copied = iv.copied_until.get().is_some_and(|t| Instant::now() < t);
        let key = (
            *states,
            current,
            [iv.requested[0].get(), iv.requested[1].get(), iv.requested[2].get()],
            stuck,
            stale,
            copied,
        );
        if iv.shown.get() == Some(key) {
            return;
        }
        iv.shown.set(Some(key));

        for (i, row) in iv.rows.iter().enumerate() {
            let state = states[i];
            // A stale grant only distorts the two the tap needs; Local Network
            // is not part of why the tap failed.
            let stale_row = stale && row.kind != PermissionKind::LocalNetwork;
            let (text, color) = match state {
                PermState::Granted if stale_row => (
                    "Switched on, but macOS still refused capture",
                    NSColor::systemOrangeColor(),
                ),
                PermState::Granted => ("Granted", NSColor::systemGreenColor()),
                PermState::NotChecked => {
                    ("Waiting for the steps above", NSColor::secondaryLabelColor())
                }
                PermState::Checking => ("Checking\u{2026}", NSColor::secondaryLabelColor()),
                // Deliberately not orange: this is not a fault, and colouring it
                // like one sends people hunting for a problem that may not exist.
                PermState::Unknowable => (
                    "macOS offers no way to read this one \u{2014} allow it if asked",
                    NSColor::secondaryLabelColor(),
                ),
                // Asked a while ago and still nothing. Two very different causes —
                // the user is still working through System Settings, or macOS
                // never asked at all — and the app cannot tell them apart, so it
                // states the fact and points at the footer instead of guessing.
                PermState::Missing if stuck[i] => (
                    "Still not granted \u{2014} if no prompt appeared, see below",
                    NSColor::systemOrangeColor(),
                ),
                PermState::Missing => ("Not granted yet", NSColor::systemOrangeColor()),
            };
            if row.status.stringValue().to_string() != text {
                row.status.setStringValue(&NSString::from_str(text));
            }
            row.status.setTextColor(Some(&color));

            // A stale row reads as granted, so its only useful action is the pane
            // where the entry has to be removed and re-added — the prompt is
            // spent and macOS will not re-show it for a permission it thinks it
            // already gave.
            let want_title = if stale_row || stuck[i] {
                "Open System Settings"
            } else if state == PermState::Granted {
                "Granted"
            } else if !iv.requested[i].get() {
                match row.kind {
                    PermissionKind::LocalNetwork => "Check network access",
                    _ => "Grant\u{2026}",
                }
            } else {
                "Open System Settings"
            };
            if row.button.title().to_string() != want_title {
                row.button.setTitle(&NSString::from_str(want_title));
            }
            // Unknowable keeps its button live: opening the pane is the only way
            // left for the user to see the switch for themselves.
            row.button.setEnabled(stale_row || state != PermState::Granted);
            // Exactly one Return target on screen. Clearing the others in the
            // same pass is not optional: a stale "\r" left on a finished row
            // makes Return fire two buttons at once.
            row.button.setKeyEquivalent(if i == current && current < ORDER.len() {
                ns_string!("\r")
            } else {
                ns_string!("")
            });
        }

        // Local Network never gates the restart — it cannot even be read, and
        // blocking on an unanswerable question would strand a user whose
        // permissions are all fine.
        let tap_ok = states[0] == PermState::Granted && states[1] == PermState::Granted;
        iv.restart_button.setEnabled(tap_ok);
        iv.restart_button
            .setKeyEquivalent(if tap_ok { ns_string!("\r") } else { ns_string!("") });

        // Same cure, different certainty. A failed tap PROVES the entry is stale;
        // a slow row only suggests it, so that one is phrased as a conditional
        // rather than told to a user who is merely still clicking through.
        let footer = if stale {
            "keyboard-it looks switched on above, but macOS refused capture \u{2014} that entry \
             belongs to an earlier build. Select keyboard-it in the list, remove it with \
             \u{2212}, add it back with \u{002B}, then restart."
        } else if stuck.iter().any(|s| *s) {
            "If no prompt appeared: macOS keeps one saved answer per app, and an unsigned build \
             changes identity on every update, so the row outlives the permission. Remove \
             keyboard-it from the list with \u{2212}, add it back with \u{002B}, then restart."
        } else if !tap_ok {
            "macOS only applies a permission when the app starts \u{2014} one restart at the \
             end is all that is needed."
        } else if !iv.can_relaunch {
            "Running outside an .app bundle: keyboard-it cannot restart itself, so quit and \
             start it again by hand."
        } else if states[2] == PermState::Unknowable {
            "Local Network has no API to read, so keyboard-it can only confirm it once your PC \
             answers. Nothing is wrong if it stays unread \u{2014} restart and pair as usual."
        } else {
            "All set. Restart to apply the permissions."
        };
        if iv.footer.stringValue().to_string() != footer {
            iv.footer.setStringValue(&NSString::from_str(footer));
        }

        // The System Settings dance is fiddly; `sudo tccutil reset` always works
        // and needs a root the app does not have, so hand over the exact command.
        iv.copy_button.setHidden(!any_stuck);
        let copy_title = if copied {
            "Copied \u{2713} \u{2014} run it in Terminal"
        } else {
            "Copy reset command"
        };
        if iv.copy_button.title().to_string() != copy_title {
            iv.copy_button.setTitle(&NSString::from_str(copy_title));
        }
    }
}
