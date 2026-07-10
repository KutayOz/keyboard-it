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
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEINPUT, MOUSE_EVENT_FLAGS,
        MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_WHEEL, VIRTUAL_KEY,
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
            eprintln!("SendInput queued {}/{} events (input blocked?)", sent, inputs.len());
        }
    }

    /// Build + send a single mouse INPUT (mouse counterpart of send_scancode).
    fn send_mouse(dx: i32, dy: i32, mouse_data: u32, flags: MOUSE_EVENT_FLAGS) {
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
