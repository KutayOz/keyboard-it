//! Slint-based Windows shell: native system tray + small settings window.
//! `serve` (network/injection) runs on a background thread; state reaches the UI via
//! `invoke_from_event_loop`. Start/Stop, saving settings, and autostart are managed here.

use std::cell::RefCell;
use std::rc::Rc;

use protocol::config::{Config, Role};

use crate::{autostart, serve};

slint::include_modules!();

fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

/// Build a Config from the settings window fields (without saving).
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

/// `cfg_warning`: error from loading the config at startup (if any) — release builds have
/// no console, so the user sees it here (status line).
pub fn run(cfg: Config, cfg_warning: Option<String>) -> std::io::Result<()> {
    let tray = Tray::new().map_err(io_err)?;
    let settings = SettingsWindow::new().map_err(io_err)?;

    // Fill the key field from config; if config is empty, fall back to the KEYBOARD_IT_KEY
    // env var (Save then moves env→config and the env var dependency ends).
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

    // Listener handle — lives on the UI thread (Rc, not Send; fine here).
    let listener: Rc<RefCell<Option<serve::Handle>>> = Rc::new(RefCell::new(None));

    // Start (stop first if running → restartable).
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

            // Connection status -> status line + tray (from the background thread, post
            // to the UI).
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
                                        "Connected — encrypted channel established."
                                    }
                                    serve::ConnStatus::Disconnected => {
                                        "Connection closed — listening."
                                    }
                                    // The most likely setup mistake: different keys on
                                    // the two sides. Make it visible.
                                    serve::ConnStatus::HandshakeFailed => {
                                        "Handshake failed — is the pairing key identical on both machines?"
                                    }
                                }
                                .into(),
                            );
                        }
                        // The tray shows it at a glance too: the icon, tooltip and the
                        // "Status:" menu line derive from this property (slint).
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
                        t.set_conn(0); // fresh listener: "Waiting for connection"
                    }
                    s.set_active(true);
                    s.set_status_line("Started — waiting for connection.".into());
                }
                Err(e) => {
                    if let Some(t) = tw.upgrade() {
                        t.set_active(false);
                    }
                    s.set_active(false);
                    s.set_status_line(format!("Could not start: {e}").into());
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
                s.set_status_line("Stopped.".into());
            }
        })
    };

    // --- Tray events ---
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

    // --- Window events ---
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
                    s.set_status_line("Saved.".into());
                    let running = listener.borrow().is_some();
                    if running {
                        start(); // restart with the new settings if running
                    }
                }
                Err(e) => s.set_status_line(format!("Could not save: {e}").into()),
            }
        });
    }
    {
        let sw = settings.as_weak();
        settings.on_autostart_changed(move |on| {
            let msg = match autostart::set_enabled(on) {
                Ok(_) => if on {
                    "Autostart enabled."
                } else {
                    "Autostart disabled."
                }
                .to_string(),
                Err(e) => format!("Autostart unchanged: {e}"),
            };
            if let Some(s) = sw.upgrade() {
                s.set_status_line(msg.into());
                s.set_autostart(autostart::is_enabled()); // reflect the actual state
            }
        });
    }

    // Start automatically at launch if a secret exists (config or env).
    let have_secret = !cfg.shared_secret.is_empty()
        || std::env::var("KEYBOARD_IT_KEY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
    if have_secret {
        do_start();
    }

    // If the config could not be read the user must see it: write the warning to the
    // status line and open the settings window (the tray still works normally).
    if let Some(w) = cfg_warning {
        settings.set_status_line(w.into());
        let _ = settings.show();
    }

    tray.show().map_err(io_err)?;
    slint::run_event_loop().map_err(io_err)?;
    Ok(())
}
