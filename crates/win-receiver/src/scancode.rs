//! USB HID Usage (Usage Page 0x07) -> PS/2 Set-1 make scancode eşlemesi.
//!
//! Dönen `bool` = "extended" (0xE0 önekli) tuş mu; SendInput'ta bu tuşlar için
//! KEYEVENTF_EXTENDEDKEY gerekir (oklar, sağ Ctrl/Alt, navigasyon bloğu, Win tuşları).
//! Kaynak: Microsoft "HID to PS/2 Scan Code Translation Table". Eksik tuş çıkarsa
//! buraya bir satır eklemek yeterli.

/// `None` = bu HID usage için henüz eşleme yok.
pub fn hid_to_scancode(hid: u16) -> Option<(u16, bool)> {
    let v = match hid {
        // --- Harfler a-z (0x04..=0x1D) ---
        0x04 => (0x1E, false), // a
        0x05 => (0x30, false), // b
        0x06 => (0x2E, false), // c
        0x07 => (0x20, false), // d
        0x08 => (0x12, false), // e
        0x09 => (0x21, false), // f
        0x0A => (0x22, false), // g
        0x0B => (0x23, false), // h
        0x0C => (0x17, false), // i
        0x0D => (0x24, false), // j
        0x0E => (0x25, false), // k
        0x0F => (0x26, false), // l
        0x10 => (0x32, false), // m
        0x11 => (0x31, false), // n
        0x12 => (0x18, false), // o
        0x13 => (0x19, false), // p
        0x14 => (0x10, false), // q
        0x15 => (0x13, false), // r
        0x16 => (0x1F, false), // s
        0x17 => (0x14, false), // t
        0x18 => (0x16, false), // u
        0x19 => (0x2F, false), // v
        0x1A => (0x11, false), // w
        0x1B => (0x2D, false), // x
        0x1C => (0x15, false), // y
        0x1D => (0x2C, false), // z

        // --- Rakamlar 1-0 (0x1E..=0x27) ---
        0x1E => (0x02, false), // 1
        0x1F => (0x03, false), // 2
        0x20 => (0x04, false), // 3
        0x21 => (0x05, false), // 4
        0x22 => (0x06, false), // 5
        0x23 => (0x07, false), // 6
        0x24 => (0x08, false), // 7
        0x25 => (0x09, false), // 8
        0x26 => (0x0A, false), // 9
        0x27 => (0x0B, false), // 0

        // --- Kontrol / noktalama ---
        0x28 => (0x1C, false), // Enter
        0x29 => (0x01, false), // Esc
        0x2A => (0x0E, false), // Backspace
        0x2B => (0x0F, false), // Tab
        0x2C => (0x39, false), // Space
        0x2D => (0x0C, false), // -
        0x2E => (0x0D, false), // =
        0x2F => (0x1A, false), // [
        0x30 => (0x1B, false), // ]
        0x31 => (0x2B, false), // \
        0x33 => (0x27, false), // ;
        0x34 => (0x28, false), // '
        0x35 => (0x29, false), // `
        0x36 => (0x33, false), // ,
        0x37 => (0x34, false), // .
        0x38 => (0x35, false), // /
        0x39 => (0x3A, false), // CapsLock

        // --- Navigasyon (extended) ---
        0x49 => (0x52, true), // Insert
        0x4A => (0x47, true), // Home
        0x4B => (0x49, true), // PageUp
        0x4C => (0x53, true), // Delete
        0x4D => (0x4F, true), // End
        0x4E => (0x51, true), // PageDown
        0x4F => (0x4D, true), // ArrowRight
        0x50 => (0x4B, true), // ArrowLeft
        0x51 => (0x50, true), // ArrowDown
        0x52 => (0x48, true), // ArrowUp

        // --- Modifierlar (0xE0..=0xE7) ---
        0xE0 => (0x1D, false), // LeftCtrl
        0xE1 => (0x2A, false), // LeftShift
        0xE2 => (0x38, false), // LeftAlt
        0xE3 => (0x5B, true),  // LeftGUI (Win)
        0xE4 => (0x1D, true),  // RightCtrl
        0xE5 => (0x36, false), // RightShift
        0xE6 => (0x38, true),  // RightAlt (AltGr)
        0xE7 => (0x5C, true),  // RightGUI (Win)

        // --- ISO tuşu (Türkçe/ISO: <>| ) ---
        0x64 => (0x56, false), // HID Non-US backslash -> Set-1 make 0x56 (Europe 2)

        // --- Fonksiyon tuşları F1-F12 ---
        0x3A => (0x3B, false), // F1
        0x3B => (0x3C, false), // F2
        0x3C => (0x3D, false), // F3
        0x3D => (0x3E, false), // F4
        0x3E => (0x3F, false), // F5
        0x3F => (0x40, false), // F6
        0x40 => (0x41, false), // F7
        0x41 => (0x42, false), // F8
        0x42 => (0x43, false), // F9
        0x43 => (0x44, false), // F10
        0x44 => (0x57, false), // F11 (dizi kırılır: 0x45 DEĞİL)
        0x45 => (0x58, false), // F12 (dizi kırılır: 0x46 DEĞİL)

        _ => return None,
    };
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_extended() {
        assert_eq!(hid_to_scancode(0x0B), Some((0x23, false))); // h
        assert_eq!(hid_to_scancode(0x12), Some((0x18, false))); // o
        assert_eq!(hid_to_scancode(0x4F), Some((0x4D, true)));  // ArrowRight (extended)
        assert_eq!(hid_to_scancode(0x00), None);                // eşleme yok
    }
}
