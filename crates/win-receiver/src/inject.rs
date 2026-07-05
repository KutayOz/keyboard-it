//! Enjeksiyon katmanı: bir `KeyEvent`'i işletim sistemine bas.
//!
//! Windows'ta gerçek `SendInput` (scancode). Windows dışında dry-run (yazdır).

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

    // Windows tekerlek "bir tık" birimi (WindowsAndMessaging::WHEEL_DELTA ile aynı).
    // Ekstra Cargo feature'ı gerektirmemek için gömülü.
    const WHEEL_DELTA: i32 = 120;

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

    /// Tek bir fare INPUT'u kur + gönder (send_scancode'un fare karşılığı).
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
            eprintln!("SendInput fare {}/{} (UIPI/engellendi?)", sent, inputs.len());
        }
    }

    /// RELATİF hareket. MOUSEEVENTF_ABSOLUTE ayarlanmaz -> dx/dy relatif delta (sağ/aşağı +).
    pub fn move_relative(dx: i32, dy: i32) {
        send_mouse(dx, dy, 0, MOUSEEVENTF_MOVE);
    }

    /// Tek buton geçişi (edge). button: 0=L,1=R,2=M.
    pub fn button(button: u8, down: bool) {
        let flag = match (button, down) {
            (0, true) => MOUSEEVENTF_LEFTDOWN,
            (0, false) => MOUSEEVENTF_LEFTUP,
            (1, true) => MOUSEEVENTF_RIGHTDOWN,
            (1, false) => MOUSEEVENTF_RIGHTUP,
            (2, true) => MOUSEEVENTF_MIDDLEDOWN,
            (2, false) => MOUSEEVENTF_MIDDLEUP,
            _ => return, // bilinmeyen buton: yok say
        };
        send_mouse(0, 0, 0, flag);
    }

    /// Scroll. dy>0 yukarı/ileri, dx>0 sağa. Dikey ve yatay AYRI INPUT'lardır.
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

/// Gelen bir fare olayını işle. Windows'ta SendInput; dışında dry-run.
pub fn handle_mouse(ev: InputEvent) {
    #[cfg(windows)]
    {
        match ev {
            InputEvent::MouseMove { dx, dy } => win_inject::move_relative(dx as i32, dy as i32),
            InputEvent::MouseButton { button, down } => win_inject::button(button, down),
            InputEvent::Scroll { dx, dy } => win_inject::scroll(dx, dy),
            InputEvent::Key(_) => {} // Key bu yola gelmez; main.rs ayırır
        }
    }
    #[cfg(not(windows))]
    {
        match ev {
            InputEvent::MouseMove { dx, dy } => println!("[dry-run] fare move dx={dx} dy={dy}"),
            InputEvent::MouseButton { button, down } => {
                println!("[dry-run] fare button={button} down={down}")
            }
            InputEvent::Scroll { dx, dy } => println!("[dry-run] scroll dx={dx} dy={dy}"),
            InputEvent::Key(_) => {}
        }
    }
}
