//! win-receiver: Mac'ten şifreli tuş/fare olayları alıp Windows'a SendInput ile basar.
//!
//! Windows'ta bir sistem TEPSİSİ (tray) ikonuyla çalışır (durum + Ayarlar/Cikis menüsü);
//! ağ döngüsü (serve) arka thread'de. Windows DIŞINDA (ör. macOS testi) tepsi yoktur,
//! serve doğrudan "dry-run" modunda çalışır.
//!
//! Ayarlar artık config dosyasında (protocol::config); sır config'te yoksa
//! KEYBOARD_IT_KEY env var'ına düşülür.

use std::io;

mod inject;
mod scancode;
mod serve;
#[cfg(windows)]
mod tray;

fn main() -> io::Result<()> {
    let cfg = protocol::config::Config::load()?.unwrap_or_default();

    #[cfg(windows)]
    {
        tray::run(cfg)
    }
    #[cfg(not(windows))]
    {
        serve::serve(&cfg, |_| {})
    }
}
