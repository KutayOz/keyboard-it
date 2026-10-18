//! Injection layer: press a `KeyEvent` into the operating system.
//!
//! Real `SendInput` (scancode) on Windows. Dry-run (print) elsewhere.

use protocol::{InputEvent, KeyEvent, MsgType};

use crate::scancode;

#[cfg(windows)]
mod win_inject {
    use std::mem::size_of;

    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
        MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };

    // Windows wheel "one notch" unit (same as WindowsAndMessaging::WHEEL_DELTA).
    // Inlined to avoid pulling in an extra Cargo feature.
    const WHEEL_DELTA: i32 = 120;

    /// Send a single scancode event (press/release) via SendInput.
    pub fn send_scancode(scan: u16, key_up: bool, extended: bool) {
        let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
        if key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }

        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let inputs = [input];
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            eprintln!(
                "SendInput queued {}/{} events (input blocked?)",
                sent,
                inputs.len()
            );
        }
    }

    /// Build + send a single mouse INPUT (mouse counterpart of send_scancode).
    pub(super) fn send_mouse(dx: i32, dy: i32, mouse_data: u32, flags: MOUSE_EVENT_FLAGS) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: mouse_data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [input];
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            eprintln!("SendInput mouse {}/{} (UIPI/blocked?)", sent, inputs.len());
        }
    }

    /// RELATIVE movement. MOUSEEVENTF_ABSOLUTE is not set -> dx/dy are relative deltas
    /// (right/down positive).
    pub fn move_relative(dx: i32, dy: i32) {
        send_mouse(dx, dy, 0, MOUSEEVENTF_MOVE);
    }

    /// Single button transition (edge). button: 0=L,1=R,2=M.
    pub fn button(button: u8, down: bool) {
        let flag = match (button, down) {
            (0, true) => MOUSEEVENTF_LEFTDOWN,
            (0, false) => MOUSEEVENTF_LEFTUP,
            (1, true) => MOUSEEVENTF_RIGHTDOWN,
            (1, false) => MOUSEEVENTF_RIGHTUP,
            (2, true) => MOUSEEVENTF_MIDDLEDOWN,
            (2, false) => MOUSEEVENTF_MIDDLEUP,
            _ => return, // unknown button: ignore
        };
        send_mouse(0, 0, 0, flag);
    }

    /// Scroll. dy>0 up/forward, dx>0 right. Vertical and horizontal are SEPARATE INPUTs.
    pub fn scroll(dx: i8, dy: i8) {
        if dy != 0 {
            let delta = dy as i32 * WHEEL_DELTA;
            send_mouse(0, 0, delta as u32, MOUSEEVENTF_WHEEL);
        }
        if dx != 0 {
            let delta = dx as i32 * WHEEL_DELTA;
            send_mouse(0, 0, delta as u32, MOUSEEVENTF_HWHEEL);
        }
    }
}

/// Handle an incoming key event: translate HID usage -> scancode, then press it.
pub fn handle(ev: KeyEvent) {
    let (scan, extended) = match scancode::hid_to_scancode(ev.hid_usage) {
        Some(v) => v,
        None => {
            eprintln!("no mapping: hid=0x{:04x} ({:?})", ev.hid_usage, ev.msg);
            return;
        }
    };

    // Down/Repeat -> key pressed; Up -> key released.
    let key_up = matches!(ev.msg, MsgType::Up);

    #[cfg(windows)]
    {
        win_inject::send_scancode(scan, key_up, extended);
    }
    #[cfg(not(windows))]
    {
        println!(
            "[dry-run] {:?} hid=0x{:04x} -> scancode=0x{:02x} ext={} up={}",
            ev.msg, ev.hid_usage, scan, extended, key_up
        );
    }
}

/// Handle an incoming mouse event. SendInput on Windows; dry-run elsewhere.
pub fn handle_mouse(ev: InputEvent) {
    #[cfg(windows)]
    {
        match ev {
            InputEvent::MouseMove { dx, dy } => win_inject::move_relative(dx as i32, dy as i32),
            InputEvent::MouseButton { button, down } => win_inject::button(button, down),
            InputEvent::Scroll { dx, dy } => win_inject::scroll(dx, dy),
            InputEvent::Key(_) => {} // Key never takes this path; serve.rs routes it to handle()
        }
    }
    #[cfg(not(windows))]
    {
        match ev {
            InputEvent::MouseMove { dx, dy } => println!("[dry-run] mouse move dx={dx} dy={dy}"),
            InputEvent::MouseButton { button, down } => {
                println!("[dry-run] mouse button={button} down={down}")
            }
            InputEvent::Scroll { dx, dy } => println!("[dry-run] scroll dx={dx} dy={dy}"),
            InputEvent::Key(_) => {}
        }
    }
}

/// Shared between legacy and desktop channels. Injection and release happen under
/// one lock so a superseded connection can never release the new owner's keys.
#[derive(Default)]
struct Controller {
    generation: u64,
    live: bool,
    keys: std::collections::HashSet<u16>,
    buttons: std::collections::HashSet<u8>,
}
#[cfg(test)]
pub static TEST_CONTROLLER: std::sync::Mutex<()> = std::sync::Mutex::new(());
static CONTROLLER: std::sync::OnceLock<std::sync::Mutex<Controller>> = std::sync::OnceLock::new();
fn controller() -> &'static std::sync::Mutex<Controller> {
    CONTROLLER.get_or_init(Default::default)
}
impl Controller {
    fn release(&mut self) {
        for hid in self.keys.drain() {
            handle(KeyEvent {
                msg: MsgType::Up,
                hid_usage: hid,
                modifiers: 0,
            });
        }
        for button in self.buttons.drain() {
            handle_mouse(InputEvent::MouseButton {
                button,
                down: false,
            });
        }
    }
}
pub fn acquire() -> u64 {
    let mut c = controller().lock().unwrap();
    c.release();
    c.generation += 1;
    c.live = true;
    c.generation
}
pub fn release(owner: u64) {
    let mut c = controller().lock().unwrap();
    if c.generation == owner {
        c.release();
    }
}
pub fn owned(owner: u64) -> bool {
    let c = controller().lock().unwrap();
    c.live && c.generation == owner
}
pub fn connected() -> bool {
    controller().lock().unwrap().live
}
pub fn retire(owner: u64) {
    let mut c = controller().lock().unwrap();
    if c.generation == owner {
        c.release();
        c.live = false;
    }
}
pub fn event(owner: u64, ev: InputEvent) -> bool {
    let mut c = controller().lock().unwrap();
    if !c.live || c.generation != owner {
        return false;
    }
    match ev {
        InputEvent::Key(k) => {
            if k.msg == MsgType::Up {
                c.keys.remove(&k.hid_usage);
            } else {
                c.keys.insert(k.hid_usage);
            }
            handle(k);
        }
        InputEvent::MouseButton { button, down } => {
            if down {
                c.buttons.insert(button);
            } else {
                c.buttons.remove(&button);
            }
            handle_mouse(ev);
        }
        _ => handle_mouse(ev),
    }
    true
}
#[cfg(windows)]
pub fn absolute(owner: u64, x: u16, y: u16, d: &protocol::desktop::Display) {
    let c = controller().lock().unwrap();
    if !c.live || c.generation != owner {
        return;
    }
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };
        let px = d.left as i64 + x as i64 * (d.source_width - 1) as i64 / 65535;
        let py = d.top as i64 + y as i64 * (d.source_height - 1) as i64 / 65535;
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN) as i64;
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN) as i64;
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2) as i64;
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2) as i64;
        win_inject::send_mouse(
            ((px - vx) * 65535 / (vw - 1)).clamp(0, 65535) as i32,
            ((py - vy) * 65535 / (vh - 1)).clamp(0, 65535) as i32,
            0,
            MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
        );
    }
    #[cfg(not(windows))]
    let _ = (x, y, d);
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn old_disconnect_cannot_release_or_inject_into_new_owner() {
        let _guard = TEST_CONTROLLER.lock().unwrap();
        let old = acquire();
        let new = acquire();
        assert!(!event(old, InputEvent::MouseMove { dx: 1, dy: 1 }));
        release(old);
        assert!(owned(new));
        assert!(event(new, InputEvent::MouseMove { dx: 1, dy: 1 }));
        release(new);
    }
}
