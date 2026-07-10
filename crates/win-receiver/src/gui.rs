//! Slint tabanlı Windows kabuğu: native sistem tepsisi + küçük ayar penceresi.
//! `serve` (ağ/enjeksiyon) arka thread'de; UI ile durum `invoke_from_event_loop`
//! üzerinden konuşur. Başlat/Durdur, ayar kaydet, otomatik başlat buradan yönetilir.

use std::cell::RefCell;
use std::rc::Rc;

use protocol::config::{Config, Role};

use crate::{autostart, serve};

slint::include_modules!();

fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

/// Ayar penceresindeki alanlardan bir Config kur (kaydetmeden).
fn config_from_ui(w: &SettingsWindow) -> Config {
    Config {
        shared_secret: w.get_pairing_key().trim().to_string(),
        peer_host: w.get_peer_ip().trim().to_string(),
        role: Role::Receiver,
        port: w
            .get_peer_port()
            .trim()
            .parse()
            .unwrap_or(protocol::DEFAULT_PORT),
    }
}

/// `cfg_warning`: açılışta config yüklenirken oluşan hata (varsa) — release'te
/// konsol olmadığından kullanıcıya buradan (status satırı) gösterilir.
pub fn run(cfg: Config, cfg_warning: Option<String>) -> std::io::Result<()> {
    let tray = Tray::new().map_err(io_err)?;
    let settings = SettingsWindow::new().map_err(io_err)?;

    // Anahtar alanını config'ten doldur; config boşsa env KEYBOARD_IT_KEY'e düş
    // (böylece Kaydet env→config'e taşır ve env var'a bağımlılık biter).
    let key_display = if cfg.shared_secret.is_empty() {
        std::env::var("KEYBOARD_IT_KEY").unwrap_or_default()
    } else {
        cfg.shared_secret.clone()
    };
    settings.set_pairing_key(key_display.into());
    settings.set_peer_ip(cfg.peer_host.clone().into());
    settings.set_peer_port(cfg.port.to_string().into());
    settings.set_autostart(autostart::is_enabled());
    settings.set_active(false);
    tray.set_active(false);

    // Dinleyici tutamacı — UI thread'de yaşar (Rc, Send değil; sorun yok).
    let listener: Rc<RefCell<Option<serve::Handle>>> = Rc::new(RefCell::new(None));

    // Başlat (önce varsa durdur → yeniden başlatılabilir).
    let do_start: Rc<dyn Fn()> = {
        let listener = listener.clone();
        let tw = tray.as_weak();
        let sw = settings.as_weak();
        Rc::new(move || {
            let existing = listener.borrow_mut().take();
            if let Some(mut h) = existing {
                h.stop();
            }
            let Some(s) = sw.upgrade() else { return };
            let cfg = config_from_ui(&s);
            let _ = cfg.save();

            // Bağlantı durumu -> status-line + tepsi (arka thread'den, UI'a post et).
            let on_conn = {
                let sw = sw.clone();
                let tw = tw.clone();
                move |status: serve::ConnStatus| {
                    let sw = sw.clone();
                    let tw = tw.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(s) = sw.upgrade() {
                            s.set_status_line(
                                match status {
                                    serve::ConnStatus::Connected => {
                                        "Bağlandı — şifreli kanal kuruldu."
                                    }
                                    serve::ConnStatus::Disconnected => {
                                        "Bağlantı kapandı — dinleniyor."
                                    }
                                    // Dağıtımda en olası kurulum hatası: iki
                                    // tarafa farklı anahtar yazılması. Görünür olsun.
                                    serve::ConnStatus::HandshakeFailed => {
                                        "El sıkışma başarısız — eşleşme anahtarı iki tarafta aynı mı?"
                                    }
                                }
                                .into(),
                            );
                        }
                        // Tepsi de bir bakışta göstersin: ikon + tooltip +
                        // menüdeki "Durum:" satırı bu özellikten türetilir (slint).
                        if let Some(t) = tw.upgrade() {
                            t.set_conn(match status {
                                serve::ConnStatus::Connected => 1,
                                serve::ConnStatus::Disconnected => 0,
                                serve::ConnStatus::HandshakeFailed => 2,
                            });
                        }
                    });
                }
            };

            match serve::start(&cfg, on_conn) {
                Ok(h) => {
                    *listener.borrow_mut() = Some(h);
                    if let Some(t) = tw.upgrade() {
                        t.set_active(true);
                        t.set_conn(0); // yeni dinleyici: "Bağlantı bekleniyor"
                    }
                    s.set_active(true);
                    s.set_status_line("Başlatıldı — bağlantı bekleniyor.".into());
                }
                Err(e) => {
                    if let Some(t) = tw.upgrade() {
                        t.set_active(false);
                    }
                    s.set_active(false);
                    s.set_status_line(format!("Başlatılamadı: {e}").into());
                }
            }
        })
    };

    let do_stop: Rc<dyn Fn()> = {
        let listener = listener.clone();
        let tw = tray.as_weak();
        let sw = settings.as_weak();
        Rc::new(move || {
            let existing = listener.borrow_mut().take();
            if let Some(mut h) = existing {
                h.stop();
            }
            if let Some(t) = tw.upgrade() {
                t.set_active(false);
            }
            if let Some(s) = sw.upgrade() {
                s.set_active(false);
                s.set_status_line("Durduruldu.".into());
            }
        })
    };

    // --- Tepsi olayları ---
    {
        let sw = settings.as_weak();
        tray.on_show_settings(move || {
            if let Some(s) = sw.upgrade() {
                let _ = s.show();
            }
        });
    }
    {
        let listener = listener.clone();
        let start = do_start.clone();
        let stop = do_stop.clone();
        tray.on_toggle_listener(move || {
            let running = listener.borrow().is_some();
            if running {
                stop();
            } else {
                start();
            }
        });
    }
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });

    // --- Pencere olayları ---
    {
        let start = do_start.clone();
        settings.on_start_listener(move || start());
    }
    {
        let stop = do_stop.clone();
        settings.on_stop_listener(move || stop());
    }
    {
        let sw = settings.as_weak();
        let listener = listener.clone();
        let start = do_start.clone();
        settings.on_save(move || {
            let Some(s) = sw.upgrade() else { return };
            let cfg = config_from_ui(&s);
            match cfg.save() {
                Ok(_) => {
                    s.set_status_line("Kaydedildi.".into());
                    let running = listener.borrow().is_some();
                    if running {
                        start(); // çalışıyorsa yeni ayarlarla yeniden başlat
                    }
                }
                Err(e) => s.set_status_line(format!("Kaydedilemedi: {e}").into()),
            }
        });
    }
    {
        let sw = settings.as_weak();
        settings.on_autostart_changed(move |on| {
            let msg = match autostart::set_enabled(on) {
                Ok(_) => if on {
                    "Otomatik başlatma açıldı."
                } else {
                    "Otomatik başlatma kapatıldı."
                }
                .to_string(),
                Err(e) => format!("Otomatik başlatma değişmedi: {e}"),
            };
            if let Some(s) = sw.upgrade() {
                s.set_status_line(msg.into());
                s.set_autostart(autostart::is_enabled()); // gerçek durumu yansıt
            }
        });
    }

    // Sır (config ya da env) varsa açılışta otomatik başlat.
    let have_secret = !cfg.shared_secret.is_empty()
        || std::env::var("KEYBOARD_IT_KEY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
    if have_secret {
        do_start();
    }

    // Config okunamadıysa kullanıcı mutlaka görsün: uyarıyı status satırına yaz
    // ve ayar penceresini aç (tepsi yine de normal çalışır).
    if let Some(w) = cfg_warning {
        settings.set_status_line(w.into());
        let _ = settings.show();
    }

    tray.show().map_err(io_err)?;
    slint::run_event_loop().map_err(io_err)?;
    Ok(())
}
