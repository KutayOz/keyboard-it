use protocol::{InputEvent, MsgType};
use std::io;

use std::mem::size_of;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
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
pub(crate) fn send_mouse(dx: i32, dy: i32, mouse_data: u32, flags: MOUSE_EVENT_FLAGS) {
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

pub struct Windows;
impl super::InputBackend for Windows {
    fn platform(&self) -> protocol::devices::Platform {
        protocol::devices::Platform::Windows
    }
    fn supports_text(&self) -> bool {
        true
    }
    fn text(&mut self, text: &str) -> io::Result<()> {
        let inputs = unicode_inputs(text);
        if unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } as usize != inputs.len() {
            return Err(io::Error::other("Unicode input blocked by Windows"));
        }
        Ok(())
    }
    fn event(&mut self, event: InputEvent) -> io::Result<()> {
        match event {
            InputEvent::Key(k) => {
                if let Some((scan, extended)) = crate::scancode::hid_to_scancode(k.hid_usage) {
                    send_scancode(scan, k.msg == MsgType::Up, extended);
                }
            }
            InputEvent::MouseMove { dx, dy } => move_relative(dx.into(), dy.into()),
            InputEvent::MouseButton { button: b, down } => button(b, down),
            InputEvent::Scroll { dx, dy } => scroll(dx, dy),
        }
        Ok(())
    }
}

fn unicode_inputs(text: &str) -> Vec<INPUT> {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_UNICODE;
    text.encode_utf16()
        .flat_map(|unit| {
            [false, true].map(|up| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: if up {
                            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                        } else {
                            KEYEVENTF_UNICODE
                        },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
        })
        .collect()
}
#[cfg(test)]
mod unicode_tests {
    use super::*;
    #[test]
    fn unicode_packets_include_turkish_and_surrogate_pairs_with_balanced_keyups() {
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_UNICODE;
        let input = unicode_inputs("İ👋");
        assert_eq!(input.len(), 6);
        for (pair, unit) in input.chunks_exact(2).zip("İ👋".encode_utf16()) {
            unsafe {
                assert_eq!(pair[0].Anonymous.ki.wScan, unit);
                assert_eq!(pair[1].Anonymous.ki.wScan, unit);
                assert_eq!(pair[0].Anonymous.ki.wVk.0, 0);
                assert_eq!(pair[0].Anonymous.ki.dwFlags, KEYEVENTF_UNICODE);
                assert_eq!(
                    pair[1].Anonymous.ki.dwFlags,
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                );
            }
        }
    }
}
