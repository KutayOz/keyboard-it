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
    // Bozuk config.toml'da sessizce ölme (release'te konsol yok → çift tıklama
    // "hiçbir şey olmuyor" gibi görünürdü): varsayılanlara düş, hatayı GUI'ye taşı.
    let (cfg, cfg_err) = match protocol::config::Config::load() {
        Ok(c) => (c.unwrap_or_default(), None),
        Err(e) => (
            protocol::config::Config::default(),
            Some(format!("config.toml okunamadı, varsayılanlar yüklendi: {e}")),
        ),
    };

    #[cfg(windows)]
    {
        if !single_instance() {
            return Ok(()); // zaten çalışıyor
        }
        gui::run(cfg, cfg_err)
    }
    #[cfg(not(windows))]
    {
        if let Some(w) = &cfg_err {
            eprintln!("uyarı: {w}");
        }
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
