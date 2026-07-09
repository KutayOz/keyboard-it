//! win-receiver: Mac'ten şifreli tuş/fare olayları alıp Windows'a SendInput ile basar.
//!
//! Windows'ta **Slint** tabanlı sistem tepsisi + küçük ayar penceresiyle çalışır (`gui`);
//! ağ döngüsü (`serve`) arka thread'de, GUI'den Başlat/Durdur edilebilir. Ayarlar (anahtar,
//! peer IP/port) GUI'den girilir; artık config dosyası/Notepad açılmaz. Windows DIŞINDA
//! (macOS dry-run testi) tepsi yoktur, `serve` doğrudan çalışır.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io;

mod inject;
mod scancode;
mod serve;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod gui;

fn main() -> io::Result<()> {
    let cfg = protocol::config::Config::load()?.unwrap_or_default();

    #[cfg(windows)]
    {
        if !single_instance() {
            return Ok(()); // zaten çalışıyor
        }
        gui::run(cfg)
    }
    #[cfg(not(windows))]
    {
        serve::serve(&cfg, |_| {})
    }
}

/// Tek örnek koruması: adlandırılmış mutex. Zaten varsa `false` döner.
#[cfg(windows)]
fn single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        match CreateMutexW(None, false, w!("Local\\keyboard-it-singleton")) {
            // Handle kapatılmaz (HANDLE Drop değil) → mutex süreç ömrü boyunca yaşar.
            Ok(_h) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => true, // mutex kurulamadıysa engelleme
        }
    }
}
