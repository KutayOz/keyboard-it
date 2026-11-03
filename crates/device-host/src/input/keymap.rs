//! Physical position maps. The Mac map reverses the existing controller's
//! Cmd/Ctrl compatibility mapping, preserving Mac-to-Mac shortcuts.
#[cfg(any(target_os = "macos", test))]
pub fn hid_to_mac(hid: u16) -> Option<u16> {
    Some(match hid {
        0x04 => 0x00, // A
        0x05 => 0x0B, // B
        0x06 => 0x08, // C
        0x07 => 0x02, // D
        0x08 => 0x0E, // E
        0x09 => 0x03, // F
        0x0A => 0x05, // G
        0x0B => 0x04, // H
        0x0C => 0x22, // I
        0x0D => 0x26, // J
        0x0E => 0x28, // K
        0x0F => 0x25, // L
        0x10 => 0x2E, // M
        0x11 => 0x2D, // N
        0x12 => 0x1F, // O
        0x13 => 0x23, // P
        0x14 => 0x0C, // Q
        0x15 => 0x0F, // R
        0x16 => 0x01, // S
        0x17 => 0x11, // T
        0x18 => 0x20, // U
        0x19 => 0x09, // V
        0x1A => 0x0D, // W
        0x1B => 0x07, // X
        0x1C => 0x10, // Y
        0x1D => 0x06, // Z
        0x1E => 0x12, // 1
        0x1F => 0x13, // 2
        0x20 => 0x14, // 3
        0x21 => 0x15, // 4
        0x22 => 0x17, // 5
        0x23 => 0x16, // 6
        0x24 => 0x1A, // 7
        0x25 => 0x1C, // 8
        0x26 => 0x19, // 9
        0x27 => 0x1D, // 0
        0x28 => 0x24, // Return
        0x29 => 0x35, // Escape
        0x2A => 0x33, // Delete (Backspace)
        0x2B => 0x30, // Tab
        0x2C => 0x31, // Space
        0x2D => 0x1B, // -
        0x2E => 0x18, // =
        0x2F => 0x21, // [
        0x30 => 0x1E, // ]
        0x31 => 0x2A, // \
        0x33 => 0x29, // ;
        0x34 => 0x27, // '
        0x35 => 0x32, // `
        0x36 => 0x2B, // ,
        0x37 => 0x2F, // .
        0x38 => 0x2C, // /
        0x39 => 0x39, // CapsLock
        0x49 => 0x72, // Help/Insert
        0x4A => 0x73, // Home
        0x4B => 0x74, // PageUp
        0x4C => 0x75, // ForwardDelete
        0x4D => 0x77, // End
        0x4E => 0x79, // PageDown
        0x4F => 0x7C, // ArrowRight
        0x50 => 0x7B, // ArrowLeft
        0x51 => 0x7D, // ArrowDown
        0x52 => 0x7E, // ArrowUp
        0x64 => 0x0A, // kVK_ISO_Section -> HID Non-US backslash and pipe
        0x3A => 0x7A, // F1
        0x3B => 0x78, // F2
        0x3C => 0x63, // F3
        0x3D => 0x76, // F4
        0x3E => 0x60, // F5
        0x3F => 0x61, // F6
        0x40 => 0x62, // F7
        0x41 => 0x64, // F8  (mac keycode 0x64 = F8; the ISO key is keycode 0x0A, not 0x64)
        0x42 => 0x65, // F9
        0x43 => 0x6D, // F10
        0x44 => 0x67, // F11
        0x45 => 0x6F, // F12
        0xE0 => 0x37, // LeftCommand  -> Windows LeftControl
        0xE3 => 0x3B, // LeftControl  -> Windows LeftGUI/Win
        0xE1 => 0x38, // LeftShift
        0xE2 => 0x3A, // LeftOption (Alt)
        0xE4 => 0x36, // RightCommand -> Windows RightControl
        0xE7 => 0x3E, // RightControl -> Windows RightGUI/Win
        0xE5 => 0x3C, // RightShift
        0xE6 => 0x3D, // RightOption (AltGr)
        _ => return None,
    })
}

#[cfg(any(target_os = "linux", test))]
pub fn xkb_name(hid: u16) -> Option<&'static [u8; 4]> {
    Some(match hid {
        0x04 => b"AC01",
        0x05 => b"AB05",
        0x06 => b"AB03",
        0x07 => b"AC03",
        0x08 => b"AD03",
        0x09 => b"AC04",
        0x0a => b"AC05",
        0x0b => b"AC06",
        0x0c => b"AD08",
        0x0d => b"AC07",
        0x0e => b"AC08",
        0x0f => b"AC09",
        0x10 => b"AB07",
        0x11 => b"AB06",
        0x12 => b"AD09",
        0x13 => b"AD10",
        0x14 => b"AD01",
        0x15 => b"AD04",
        0x16 => b"AC02",
        0x17 => b"AD05",
        0x18 => b"AD07",
        0x19 => b"AB04",
        0x1a => b"AD02",
        0x1b => b"AB02",
        0x1c => b"AD06",
        0x1d => b"AB01",
        0x1e => b"AE01",
        0x1f => b"AE02",
        0x20 => b"AE03",
        0x21 => b"AE04",
        0x22 => b"AE05",
        0x23 => b"AE06",
        0x24 => b"AE07",
        0x25 => b"AE08",
        0x26 => b"AE09",
        0x27 => b"AE10",
        0x28 => b"RTRN",
        0x29 => b"ESC\0",
        0x2a => b"BKSP",
        0x2b => b"TAB\0",
        0x2c => b"SPCE",
        0x2d => b"AE11",
        0x2e => b"AE12",
        0x2f => b"AD11",
        0x30 => b"AD12",
        0x31 => b"BKSL",
        0x32 => b"BKSL",
        0x33 => b"AC10",
        0x34 => b"AC11",
        0x35 => b"TLDE",
        0x36 => b"AB08",
        0x37 => b"AB09",
        0x38 => b"AB10",
        0x39 => b"CAPS",
        0x3a => b"FK01",
        0x3b => b"FK02",
        0x3c => b"FK03",
        0x3d => b"FK04",
        0x3e => b"FK05",
        0x3f => b"FK06",
        0x40 => b"FK07",
        0x41 => b"FK08",
        0x42 => b"FK09",
        0x43 => b"FK10",
        0x44 => b"FK11",
        0x45 => b"FK12",
        0x49 => b"INS\0",
        0x4a => b"HOME",
        0x4b => b"PGUP",
        0x4c => b"DELE",
        0x4d => b"END\0",
        0x4e => b"PGDN",
        0x4f => b"RGHT",
        0x50 => b"LEFT",
        0x51 => b"DOWN",
        0x52 => b"UP\0\0",
        0x64 => b"LSGT",
        0xe0 => b"LCTL",
        0xe1 => b"LFSH",
        0xe2 => b"LALT",
        0xe3 => b"LWIN",
        0xe4 => b"RCTL",
        0xe5 => b"RTSH",
        0xe6 => b"RALT",
        0xe7 => b"RWIN",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positions_keep_iso_navigation_and_mac_shortcuts_distinct() {
        assert_eq!(hid_to_mac(0xe0), Some(0x37)); // controller Cmd -> target Cmd
        assert_eq!(hid_to_mac(0xe3), Some(0x3b)); // controller Ctrl -> target Ctrl
        assert_eq!(hid_to_mac(0x64), Some(0x0a));
        assert_eq!(xkb_name(0x64), Some(b"LSGT"));
        assert_eq!(xkb_name(0x41), Some(b"FK08"));
        assert_eq!(xkb_name(0x4f), Some(b"RGHT"));
        assert_eq!(hid_to_mac(0xffff), None);
        assert_eq!(xkb_name(0xffff), None);
    }
}
