//! Enjeksiyon katmanı: bir `KeyEvent`'i işletim sistemine bas.
//!
//! Windows'ta gerçek `SendInput` (scancode). Windows dışında dry-run (yazdır).

use protocol::{KeyEvent, MsgType};

use crate::scancode;

#[cfg(windows)]
mod win_inject {
    use std::mem::size_of;

    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, VIRTUAL_KEY,
    };

    /// Tek bir scancode olayını (bas/bırak) SendInput ile gönder.
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
            eprintln!("SendInput {}/{} olay ekledi (girdi engellendi mi?)", sent, inputs.len());
        }
    }
}

/// Gelen bir tuş olayını işle: HID usage -> scancode çevir, sonra bas.
pub fn handle(ev: KeyEvent) {
    let (scan, extended) = match scancode::hid_to_scancode(ev.hid_usage) {
        Some(v) => v,
        None => {
            eprintln!("eşleme yok: hid=0x{:04x} ({:?})", ev.hid_usage, ev.msg);
            return;
        }
    };

    // Down/Repeat -> tuş basılı; Up -> tuş bırakıldı.
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
