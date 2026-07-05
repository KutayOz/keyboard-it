//! macOS sanal keycode (kVK_*) -> USB HID Usage (Usage Page 0x07) eşlemesi.
//!
//! Konum-tabanlı: keycode fiziksel tuş konumunu adresler, HID usage da öyle. Böylece
//! Windows tarafı kendi düzenini uygular (bkz. win-receiver/scancode.rs). Fn tuşu (0x3F)
//! standart bir HID usage'a sahip DEĞİLDİR — `None` döner, M3'te toggle için ayrı ele alınır.

pub fn mac_keycode_to_hid(kc: i64) -> Option<u16> {
    let v: u16 = match kc {
        // Harfler
        0x00 => 0x04, // A
        0x0B => 0x05, // B
        0x08 => 0x06, // C
        0x02 => 0x07, // D
        0x0E => 0x08, // E
        0x03 => 0x09, // F
        0x05 => 0x0A, // G
        0x04 => 0x0B, // H
        0x22 => 0x0C, // I
        0x26 => 0x0D, // J
        0x28 => 0x0E, // K
        0x25 => 0x0F, // L
        0x2E => 0x10, // M
        0x2D => 0x11, // N
        0x1F => 0x12, // O
        0x23 => 0x13, // P
        0x0C => 0x14, // Q
        0x0F => 0x15, // R
        0x01 => 0x16, // S
        0x11 => 0x17, // T
        0x20 => 0x18, // U
        0x09 => 0x19, // V
        0x0D => 0x1A, // W
        0x07 => 0x1B, // X
        0x10 => 0x1C, // Y
        0x06 => 0x1D, // Z

        // Rakamlar (üst sıra)
        0x12 => 0x1E, // 1
        0x13 => 0x1F, // 2
        0x14 => 0x20, // 3
        0x15 => 0x21, // 4
        0x17 => 0x22, // 5
        0x16 => 0x23, // 6
        0x1A => 0x24, // 7
        0x1C => 0x25, // 8
        0x19 => 0x26, // 9
        0x1D => 0x27, // 0

        // Kontrol / noktalama
        0x24 => 0x28, // Return
        0x35 => 0x29, // Escape
        0x33 => 0x2A, // Delete (Backspace)
        0x30 => 0x2B, // Tab
        0x31 => 0x2C, // Space
        0x1B => 0x2D, // -
        0x18 => 0x2E, // =
        0x21 => 0x2F, // [
        0x1E => 0x30, // ]
        0x2A => 0x31, // \
        0x29 => 0x33, // ;
        0x27 => 0x34, // '
        0x32 => 0x35, // `
        0x2B => 0x36, // ,
        0x2F => 0x37, // .
        0x2C => 0x38, // /
        0x39 => 0x39, // CapsLock

        // Navigasyon
        0x72 => 0x49, // Help/Insert
        0x73 => 0x4A, // Home
        0x74 => 0x4B, // PageUp
        0x75 => 0x4C, // ForwardDelete
        0x77 => 0x4D, // End
        0x79 => 0x4E, // PageDown
        0x7C => 0x4F, // ArrowRight
        0x7B => 0x50, // ArrowLeft
        0x7D => 0x51, // ArrowDown
        0x7E => 0x52, // ArrowUp

        // ISO ek tuşu (Türkçe/ISO MacBook: Sol Shift ile Z arasındaki <>| tuşu)
        0x0A => 0x64, // kVK_ISO_Section -> HID Non-US backslash and pipe

        // Fonksiyon tuşları F1-F12 (macOS keycode'ları ardışık DEĞİL)
        0x7A => 0x3A, // F1
        0x78 => 0x3B, // F2
        0x63 => 0x3C, // F3
        0x76 => 0x3D, // F4
        0x60 => 0x3E, // F5
        0x61 => 0x3F, // F6
        0x62 => 0x40, // F7
        0x64 => 0x41, // F8  (mac keycode 0x64 = F8; ISO tuşu keycode 0x0A'dır, 0x64 değil)
        0x65 => 0x42, // F9
        0x6D => 0x43, // F10
        0x67 => 0x44, // F11
        0x6F => 0x45, // F12

        // Modifierlar — Cmd<->Ctrl TAKAS (kullanıcı tercihi: Cmd+C => Windows'ta Ctrl+C)
        0x37 => 0xE0, // LeftCommand  -> Windows LeftControl  (eskiden 0xE3)
        0x3B => 0xE3, // LeftControl  -> Windows LeftGUI/Win  (eskiden 0xE0)
        0x38 => 0xE1, // LeftShift
        0x3A => 0xE2, // LeftOption (Alt)
        0x36 => 0xE4, // RightCommand -> Windows RightControl (eskiden 0xE7)
        0x3E => 0xE7, // RightControl -> Windows RightGUI/Win (eskiden 0xE4)
        0x3C => 0xE5, // RightShift
        0x3D => 0xE6, // RightOption (AltGr)

        _ => return None,
    };
    Some(v)
}
