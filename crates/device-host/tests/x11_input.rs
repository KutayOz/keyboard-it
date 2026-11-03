#![cfg(target_os = "linux")]
//! Run only inside the dedicated Xvfb fixture in CI; never uses a physical display.
use device_host::inject;
use protocol::{InputEvent, KeyEvent, MsgType};
use std::{
    ptr, thread,
    time::{Duration, Instant},
};
use x11_dl::xlib;

#[test]
#[ignore = "requires KEYBOARD_IT_VIRTUAL_DISPLAY=1 and an isolated Xvfb server"]
fn native_x11_events_and_ownership_cleanup_reach_the_virtual_server() {
    assert_eq!(
        std::env::var("KEYBOARD_IT_VIRTUAL_DISPLAY").as_deref(),
        Ok("1")
    );
    let x = xlib::Xlib::open().unwrap();
    // The adapter initializes Xlib threading before this observer opens a display.
    inject::enable_native().unwrap();
    let display = unsafe { (x.XOpenDisplay)(ptr::null()) };
    assert!(!display.is_null());
    let code = unsafe { (x.XKeysymToKeycode)(display, 0x61) }; // XK_a, fixture is US layout
    assert_ne!(code, 0);
    let held = || unsafe {
        let mut keys = [0i8; 32];
        (x.XQueryKeymap)(display, keys.as_mut_ptr());
        let (mut root, mut child, mut px, mut py, mut wx, mut wy, mut mask) = (0, 0, 0, 0, 0, 0, 0);
        (x.XQueryPointer)(
            display,
            (x.XDefaultRootWindow)(display),
            &mut root,
            &mut child,
            &mut px,
            &mut py,
            &mut wx,
            &mut wy,
            &mut mask,
        );
        (
            keys[code as usize / 8] as u8 & (1 << (code % 8)) != 0,
            mask & xlib::Button1Mask != 0,
        )
    };
    let wait_for = |expected| {
        let deadline = Instant::now() + Duration::from_secs(2);
        while held() != expected {
            assert!(
                Instant::now() < deadline,
                "X11 input did not settle to {expected:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    };
    let old = inject::acquire();
    assert!(inject::event(
        old,
        InputEvent::Key(KeyEvent {
            msg: MsgType::Down,
            hid_usage: 4,
            modifiers: 0
        })
    ));
    assert!(inject::event(
        old,
        InputEvent::MouseButton {
            button: 0,
            down: true
        }
    ));
    wait_for((true, true));
    let new = inject::acquire();
    wait_for((false, false));
    assert!(!inject::event(
        old,
        InputEvent::Key(KeyEvent {
            msg: MsgType::Down,
            hid_usage: 4,
            modifiers: 0
        })
    ));
    assert!(inject::event(new, InputEvent::MouseMove { dx: 12, dy: 8 }));
    inject::retire(new);
    wait_for((false, false));
    unsafe {
        (x.XCloseDisplay)(display);
    }
}
