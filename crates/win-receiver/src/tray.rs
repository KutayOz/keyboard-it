//! Windows sistem tepsisi (system tray) — tao olay döngüsü + tray-icon.
//!
//! Mimari: tao EventLoop ANA thread'de (Win32 message pump); TCP `serve()` döngüsü
//! arka thread'de. Bağlantı durumu (AKTIF/PASIF) proxy ile UI thread'ine itilir ve
//! tooltip'e yansır. Menü: "Ayarlar..." (config.toml'u aç) + "Cikis".
//!
//! Yalnızca Windows'ta derlenir/çalışır (Cargo cfg(windows) bağımlılıkları).

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::serve::serve;

enum UserEvent {
    Menu(MenuEvent),
    Status(bool), // true = AKTIF (bir client bağlı), false = PASIF
}

/// Basit gömülü ikon (16x16 dolu kare). Gerçek .ico Faz 3'te.
fn placeholder_icon() -> Icon {
    let (w, h) = (16u32, 16u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[0x2e, 0x7d, 0x32, 0xff]); // yeşilimsi
    }
    Icon::from_rgba(rgba, w, h).expect("ikon oluşturulamadı")
}

pub fn run(cfg: protocol::config::Config) -> std::io::Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Menü olaylarını olay döngüsüne ilet.
    MenuEvent::set_event_handler(Some({
        let p = proxy.clone();
        move |e| {
            let _ = p.send_event(UserEvent::Menu(e));
        }
    }));

    // Menü (muda, tray_icon::menu re-export'undan).
    let menu = Menu::new();
    let item_settings = MenuItem::new("Ayarlar...", true, None);
    let item_quit = MenuItem::new("Cikis", true, None);
    menu.append_items(&[
        &item_settings,
        &PredefinedMenuItem::separator(),
        &item_quit,
    ])
    .expect("menü kurulamadı");
    let settings_id = item_settings.id().clone();
    let quit_id = item_quit.id().clone();

    // Tray, olay döngüsüyle AYNI thread'de kurulmalı.
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("keyboard-it — PASIF")
        .with_icon(placeholder_icon())
        .build()
        .expect("tray ikonu oluşturulamadı");

    // Ağ (serve) arka thread'de; durumu proxy ile UI'a it.
    {
        let p = proxy.clone();
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            let r = serve(&cfg, move |on| {
                let _ = p.send_event(UserEvent::Status(on));
            });
            if let Err(e) = r {
                eprintln!("serve hatası: {e}  (anahtar ayarlı mı? Ayarlar...)");
            }
        });
    }

    // Win32 message pump.
    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Status(on)) => {
                let _ = tray.set_tooltip(Some(if on {
                    "keyboard-it — AKTIF"
                } else {
                    "keyboard-it — PASIF"
                }));
            }
            Event::UserEvent(UserEvent::Menu(ev)) => {
                if ev.id == quit_id {
                    *control_flow = ControlFlow::Exit;
                } else if ev.id == settings_id {
                    let _ = protocol::config::Config::edit();
                }
            }
            _ => {}
        }
    });
}
