//! All AppKit access stays on the main thread. A focus check is also made on
//! every captured event, not just the timer, to avoid leaking keys across Spaces.
use super::{send, set_visible, shared};
use core_graphics::event::CallbackResult;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, EventField};
use objc2::{
    class, define_class, msg_send, rc::Retained, runtime::ProtocolObject, sel, DefinedClass,
    MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSCursor, NSEvent,
    NSPasteboard, NSPasteboardTypeString, NSTextField, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString, NSTimer,
};
use objc2_quartz_core::CALayer;
use protocol::{
    desktop::{ClipboardSync, Message},
    InputEvent, KeyEvent, MsgType,
};
use std::{
    cell::{Cell, RefCell},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

thread_local! { static VIEWER:RefCell<Option<Retained<Viewer>>>=const {RefCell::new(None)}; static FN:RefCell<(bool,Option<Instant>)>=const {RefCell::new((false,None))}; }
struct Ivars {
    window: Retained<NSWindow>,
    view: Retained<NSView>,
    layer: Retained<CALayer>,
    label: Retained<NSTextField>,
    clipboard: RefCell<ClipboardSync>,
    last_active: Cell<bool>,
    last_frame: Cell<Option<Instant>>,
    last_connection: Cell<u64>,
    last_clipboard: Cell<bool>,
    cursor_hidden: Cell<bool>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind=MainThreadOnly]
    #[name="KbItDesktopViewer"]
    #[ivars=Ivars]
    struct Viewer;
    unsafe impl NSObjectProtocol for Viewer {}
    unsafe impl NSWindowDelegate for Viewer {
        #[unsafe(method(windowDidResignKey:))]
        fn resigned(&self,_n:&NSNotification) { self.deactivate(); }
        #[unsafe(method(windowWillClose:))]
        fn closing(&self,_n:&NSNotification) {
            self.deactivate();
            NSApplication::sharedApplication(self.mtm()).setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
    }
    impl Viewer {
        #[unsafe(method(tick:))]
        fn tick(&self,_timer:&NSTimer) { self.update(); }
    }
);
impl Viewer {
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(1280., 800.));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("Windows — keyboard-it"));
        window.center();
        window.setAcceptsMouseMovedEvents(true);
        window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
        window.setBackgroundColor(Some(&NSColor::blackColor()));
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect);
        view.setWantsLayer(true);
        let layer: Retained<CALayer> =
            unsafe { msg_send![class!(AVSampleBufferDisplayLayer), new] };
        unsafe {
            let _: () =
                msg_send![&*layer,setVideoGravity:ns_string!("AVLayerVideoGravityResizeAspect")];
        }
        view.setLayer(Some(&layer));
        window.setContentView(Some(&view));
        let label =
            NSTextField::labelWithString(ns_string!("Windows bağlantısı hazırlanıyor…"), mtm);
        label.setTextColor(Some(&NSColor::whiteColor()));
        label.setFrame(NSRect::new(NSPoint::new(20., 20.), NSSize::new(1100., 22.)));
        view.addSubview(&label);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            window: window.clone(),
            view,
            layer,
            label,
            clipboard: RefCell::new(ClipboardSync::default()),
            last_active: Cell::new(false),
            last_frame: Cell::new(None),
            last_connection: Cell::new(0),
            last_clipboard: Cell::new(true),
            cursor_hidden: Cell::new(false),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        window.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        unsafe {
            let _ = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                1. / 30.,
                &this,
                sel!(tick:),
                None,
                true,
            );
        }
        this
    }
    fn focused(&self) -> bool {
        let w = &self.ivars().window;
        w.isVisible()
            && !w.isMiniaturized()
            && w.isKeyWindow()
            && w.isOnActiveSpace()
            && NSApplication::sharedApplication(self.mtm()).isActive()
    }
    fn cursor(&self, hidden: bool) {
        if self.ivars().cursor_hidden.replace(hidden) != hidden {
            if hidden {
                NSCursor::hide();
            } else {
                NSCursor::unhide();
            }
        }
    }
    fn pointer(&self) -> Option<(u16, u16)> {
        let iv = self.ivars();
        let screen_point = NSEvent::mouseLocation();
        if NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(screen_point, 0, self.mtm())
            != iv.window.windowNumber()
        {
            return None;
        }
        let point = iv
            .view
            .convertPoint_fromView(iv.window.convertPointFromScreen(screen_point), None);
        let bounds = iv.view.bounds();
        let display = shared().snapshot.lock().unwrap().display.clone();
        display.and_then(|d| {
            protocol::desktop::map_pointer(
                point.x,
                point.y,
                bounds.size.width,
                bounds.size.height,
                d.width,
                d.height,
            )
        })
    }
    fn deactivate(&self) {
        self.cursor(false);
        set_visible(false);
        self.ivars().last_active.set(false);
        self.ivars().last_frame.set(None);
        self.ivars().clipboard.borrow_mut().reset(read_clipboard());
        // Retain the last rendered image for Mission Control's Space thumbnail;
        // ready=false still prevents input until a fresh frame arrives on return.
    }
    fn update(&self) {
        let s = shared();
        let iv = self.ivars();
        let active = self.focused()
            && s.desktop_mode.load(Ordering::SeqCst)
            && !s.paused.load(Ordering::SeqCst);
        if active != iv.last_active.replace(active) {
            iv.clipboard.borrow_mut().reset(read_clipboard());
            s.snapshot.lock().unwrap().received_clipboard = None;
            set_visible(active);
            if !active {
                self.deactivate();
            }
        }
        let epoch = s.snapshot.lock().unwrap().epoch;
        let share = s.clipboard.load(Ordering::SeqCst);
        if iv.last_connection.replace(epoch) != epoch || iv.last_clipboard.replace(share) != share {
            iv.clipboard.borrow_mut().reset(read_clipboard());
            s.snapshot.lock().unwrap().received_clipboard = None;
            iv.last_frame.set(None);
        }
        iv.layer.setFrame(iv.view.bounds());
        if active {
            if let Some(frame) = s.frame.lock().unwrap().take() {
                match frame.sample() {
                    Ok(sample) => unsafe {
                        let ready: bool = msg_send![&*iv.layer, isReadyForMoreMediaData];
                        if !ready {
                            let _: () = msg_send![&*iv.layer, flush];
                        }
                        let _: () = msg_send![&*iv.layer,enqueueSampleBuffer:sample.0];
                        iv.last_frame.set(Some(Instant::now()));
                        let mut snap = s.snapshot.lock().unwrap();
                        if snap.connected {
                            snap.ready = true;
                        }
                    },
                    Err(e) => {
                        s.snapshot.lock().unwrap().status = e;
                        s.snapshot.lock().unwrap().ready = false;
                    }
                }
            }
            // A live TCP connection is not enough: never type into a frozen picture.
            if iv
                .last_frame
                .get()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(3))
            {
                s.snapshot.lock().unwrap().ready = false;
                send(Message::ReleaseAll);
                s.snapshot.lock().unwrap().status = "Görüntü durdu — yeniden bağlanılıyor…".into();
                super::disconnect();
                iv.last_frame.set(None);
            }
            if s.clipboard.load(Ordering::SeqCst) && s.snapshot.lock().unwrap().ready {
                let remote = s.snapshot.lock().unwrap().received_clipboard.take();
                if let Some((id, text)) = remote {
                    if iv.clipboard.borrow_mut().remote(id, &text) && !write_clipboard(&text) {
                        iv.clipboard.borrow_mut().reset(read_clipboard());
                    }
                }
                if let Some(text) = iv.clipboard.borrow_mut().local_change(read_clipboard()) {
                    static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                    if let Ok(chunks) = protocol::desktop::clipboard_chunks(
                        ID.fetch_add(1, Ordering::Relaxed),
                        &text,
                    ) {
                        for c in chunks {
                            send(c);
                        }
                    }
                }
            }
        }
        let snap = s.snapshot.lock().unwrap();
        let cursor_active =
            active && snap.ready && snap.connected && s.input_enabled.load(Ordering::SeqCst);
        if let Some(d) = &snap.display {
            iv.window.setTitle(&NSString::from_str(&format!(
                "Windows — {} × {} — keyboard-it",
                d.width, d.height
            )));
        }
        let text = if !s.input_enabled.load(Ordering::SeqCst) {
            "Kontrol bırakıldı — çift Fn ile geri al"
        } else {
            &snap.status
        };
        iv.label.setStringValue(&NSString::from_str(text));
        iv.label
            .setHidden(active && snap.ready && s.input_enabled.load(Ordering::SeqCst));
        drop(snap);
        self.cursor(cursor_active && self.pointer().is_some());
    }
}
pub fn release_ui() {
    VIEWER.with(|slot| {
        if let Some(v) = slot.borrow().as_ref() {
            v.deactivate();
        }
    });
}
pub fn open(mtm: MainThreadMarker) {
    if !shared().desktop_mode.load(Ordering::SeqCst) {
        super::toggle_mode();
    }
    let viewer = VIEWER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(Viewer::create(mtm));
        }
        slot.as_ref().unwrap().clone()
    });
    let w = &viewer.ivars().window;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    unsafe {
        let _: () = msg_send![&*app,activateIgnoringOtherApps:true];
    }
    w.makeKeyAndOrderFront(None);
    if !w.styleMask().contains(NSWindowStyleMask::FullScreen) {
        w.toggleFullScreen(None);
    }
}
fn read_clipboard() -> Option<String> {
    unsafe {
        NSPasteboard::generalPasteboard()
            .stringForType(NSPasteboardTypeString)
            .map(|s| s.to_string())
            .filter(|s| s.len() <= protocol::desktop::MAX_CLIPBOARD)
    }
}
fn write_clipboard(text: &str) -> bool {
    if text.contains('\0') {
        return false;
    }
    let p = NSPasteboard::generalPasteboard();
    p.clearContents();
    unsafe { p.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) }
}
/// Desktop mode owns event routing. Legacy Fn capture is used only in input-only mode.
pub fn handle_event(kind: CGEventType, event: &CGEvent) -> Option<CallbackResult> {
    let s = shared();
    if s.paused.load(Ordering::SeqCst) {
        return Some(CallbackResult::Keep);
    }
    if !s.desktop_mode.load(Ordering::SeqCst) {
        return None;
    }
    Some(VIEWER.with(|slot| {
        let slot = slot.borrow();
        let Some(v) = slot.as_ref() else {
            return CallbackResult::Keep;
        };
        if !v.focused() || s.paused.load(Ordering::SeqCst) {
            if s.visible.load(Ordering::SeqCst) {
                v.deactivate();
            }
            return CallbackResult::Keep;
        }
        let kc = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
        if matches!(kind, CGEventType::FlagsChanged) && kc == 0x3f {
            let down = event
                .get_flags()
                .contains(CGEventFlags::CGEventFlagSecondaryFn);
            FN.with(|state| {
                let mut st = state.borrow_mut();
                if down && !st.0 {
                    if st
                        .1
                        .is_some_and(|t| t.elapsed() < Duration::from_millis(400))
                    {
                        let enabled = !s.input_enabled.load(Ordering::SeqCst);
                        s.input_enabled.store(enabled, Ordering::SeqCst);
                        send(Message::ReleaseAll);
                        st.1 = None;
                    } else {
                        st.1 = Some(Instant::now());
                    }
                }
                st.0 = down;
            });
            return CallbackResult::Drop;
        }
        if !s.input_enabled.load(Ordering::SeqCst)
            || !{
                let snap = s.snapshot.lock().unwrap();
                snap.ready && snap.connected
            }
        {
            return CallbackResult::Keep;
        }
        // Mission Control and Space-switch keyboard shortcuts stay on the Mac.
        if event.get_flags().contains(CGEventFlags::CGEventFlagControl) && matches!(kc, 123..=126) {
            send(Message::ReleaseAll);
            return CallbackResult::Keep;
        }
        match kind {
            CGEventType::KeyDown | CGEventType::KeyUp => {
                if let Some(hid) = crate::keymap::mac_keycode_to_hid(kc) {
                    let msg = if matches!(kind, CGEventType::KeyUp) {
                        MsgType::Up
                    } else {
                        MsgType::Down
                    };
                    send(Message::Input(InputEvent::Key(KeyEvent {
                        msg,
                        hid_usage: hid,
                        modifiers: 0,
                    })));
                }
                CallbackResult::Drop
            }
            CGEventType::FlagsChanged => {
                if let Some(hid) = crate::keymap::mac_keycode_to_hid(kc) {
                    if kc == 0x39 {
                        for msg in [MsgType::Down, MsgType::Up] {
                            send(Message::Input(InputEvent::Key(KeyEvent {
                                msg,
                                hid_usage: hid,
                                modifiers: 0,
                            })));
                        }
                    } else if let Some(mask) = crate::capture::modifier_mask(kc) {
                        let msg = if event.get_flags().contains(mask) {
                            MsgType::Down
                        } else {
                            MsgType::Up
                        };
                        send(Message::Input(InputEvent::Key(KeyEvent {
                            msg,
                            hid_usage: hid,
                            modifiers: 0,
                        })));
                    }
                }
                CallbackResult::Drop
            }
            _ => {
                let Some((x, y)) = v.pointer() else {
                    // A drag may end in the letterbox or titlebar. Its release
                    // must still reach Windows even though the pointer is outside.
                    let button = match kind {
                        CGEventType::LeftMouseUp => Some(0),
                        CGEventType::RightMouseUp => Some(1),
                        CGEventType::OtherMouseUp => Some(2),
                        _ => None,
                    };
                    if let Some(button) = button {
                        send(Message::Input(InputEvent::MouseButton {
                            button,
                            down: false,
                        }));
                    }
                    v.cursor(false);
                    return CallbackResult::Keep;
                };
                v.cursor(true);
                send(Message::Absolute { x, y });
                match kind {
                    CGEventType::LeftMouseDown | CGEventType::LeftMouseUp => {
                        send(Message::Input(InputEvent::MouseButton {
                            button: 0,
                            down: matches!(kind, CGEventType::LeftMouseDown),
                        }))
                    }
                    CGEventType::RightMouseDown | CGEventType::RightMouseUp => {
                        send(Message::Input(InputEvent::MouseButton {
                            button: 1,
                            down: matches!(kind, CGEventType::RightMouseDown),
                        }))
                    }
                    CGEventType::OtherMouseDown | CGEventType::OtherMouseUp
                        if event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER)
                            == 2 =>
                    {
                        send(Message::Input(InputEvent::MouseButton {
                            button: 2,
                            down: matches!(kind, CGEventType::OtherMouseDown),
                        }))
                    }
                    CGEventType::ScrollWheel => {
                        let dx = event
                            .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2)
                            .clamp(-128, 127) as i8;
                        let dy = event
                            .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1)
                            .clamp(-128, 127) as i8;
                        send(Message::Input(InputEvent::Scroll { dx, dy }));
                    }
                    _ => {}
                }
                // Keep pointer movement local so titlebar, Dock and menu bar remain
                // reachable. Clicks in the video belong only to Windows.
                if matches!(
                    kind,
                    CGEventType::MouseMoved
                        | CGEventType::LeftMouseDragged
                        | CGEventType::RightMouseDragged
                        | CGEventType::OtherMouseDragged
                ) {
                    CallbackResult::Keep
                } else {
                    CallbackResult::Drop
                }
            }
        }
    }))
}

/// Native main-thread smoke check; no windows are shown, no input tap or real
/// configuration is opened. Kept out of distribution builds.
#[cfg(debug_assertions)]
pub fn check_viewer() -> std::io::Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| std::io::Error::other("AppKit requires main thread"))?;
    let _app = NSApplication::sharedApplication(mtm);
    let viewer = Viewer::create(mtm);
    let mut decoder = super::video::Decoder::default();
    let frame = decoder
        .decode(include_bytes!("../../tests/fixtures/desktop.h264"))
        .map_err(std::io::Error::other)?
        .ok_or_else(|| std::io::Error::other("No decoded frame"))?;
    let sample = frame.sample().map_err(std::io::Error::other)?;
    unsafe {
        let _: () = msg_send![&*viewer.ivars().layer,enqueueSampleBuffer:sample.0];
        let status: isize = msg_send![&*viewer.ivars().layer, status];
        if status == 2 {
            return Err(std::io::Error::other("AVSampleBufferDisplayLayer failed"));
        }
        let _: () = msg_send![&*viewer.ivars().layer, flushAndRemoveImage];
    }
    if !viewer
        .ivars()
        .window
        .collectionBehavior()
        .contains(NSWindowCollectionBehavior::FullScreenPrimary)
    {
        return Err(std::io::Error::other(
            "Window cannot create a full-screen Space",
        ));
    }
    println!("Native viewer, VideoToolbox and display layer smoke check passed.");
    Ok(())
}
