//! Slint-based Windows shell: native system tray + small settings window.
//! `serve` (network/injection) runs on a background thread; state reaches the UI via
//! `invoke_from_event_loop`. Start/Stop, saving settings, and autostart are managed here.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::config::{Config, Role};
use protocol::pairing::{code_display, PairRequest};

use crate::diag::plog;
use crate::{autostart, serve};

slint::include_modules!();

/// How long the pairing dialog waits for a click before declining on its own.
/// The Mac gives it more than this, so a timeout surfaces there as a decline
/// rather than a dropped connection.
const PAIR_DECISION_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the pairing thread parks while the dialog is up. `Arc`/`Mutex` rather
/// than `Rc`/`RefCell` because the two ends really are on different threads: the
/// UI callbacks fill it in, the pairing thread drains it.
type AnswerSlot = Arc<Mutex<Option<mpsc::Sender<bool>>>>;

/// Answer the waiting pairing thread, at most once per request. Taking the
/// sender is what makes a second click (or a close after a click) a no-op.
///
/// `who` only exists for the trace, and it earns its place: "Allow found nothing
/// waiting" is a completely different bug from "Allow was never pressed", and
/// from the Mac the two are the same 60 s of silence.
fn reply(slot: &AnswerSlot, yes: bool, who: &str) {
    match slot.lock() {
        Ok(mut s) => match s.take() {
            Some(tx) => {
                let delivered = tx.send(yes).is_ok();
                plog!("{who} -> answer={yes}, delivered to the pairing thread: {delivered}");
            }
            // Expected for Stop when nothing is pending; a real problem if it
            // follows a click the user actually made.
            None => plog!("{who} -> no request waiting (already answered, or timed out)"),
        },
        Err(_) => plog!("{who} -> the answer slot is poisoned; request left unanswered"),
    }
}

fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

/// Fold the settings window fields into the live config.
///
/// Mutates in place rather than building a fresh `Config`: the window only owns
/// the port and knows nothing about `shared_secret` — which is generated at
/// first run and handed out by pairing. Rebuilding the struct from UI fields
/// would silently wipe it and un-pair every Mac on the next Save.
fn apply_ui(cfg: &mut Config, w: &SettingsWindow) {
    cfg.role = Role::Receiver;
    cfg.port = w
        .get_listen_port()
        .trim()
        .parse()
        .unwrap_or(protocol::DEFAULT_PORT);
}

/// `cfg_warning`: error from loading the config at startup (if any) — release builds have
/// no console, so the user sees it here (status line).
pub fn run(cfg: Config, cfg_warning: Option<String>) -> std::io::Result<()> {
    let tray = Tray::new().map_err(io_err)?;
    let settings = SettingsWindow::new().map_err(io_err)?;
    let pair_win = PairRequestWindow::new().map_err(io_err)?;

    settings.set_listen_port(cfg.port.to_string().into());
    settings.set_autostart(autostart::is_enabled());
    settings.set_active(false);
    tray.set_active(false);

    // Identity line — hostname + LAN IPv4(s) — so the user knows which entry in the
    // Mac's discovered list is this machine.
    let ips = crate::netinfo::lan_ipv4s();
    let ip_text = if ips.is_empty() {
        "no LAN IPv4 found".to_string()
    } else {
        ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>().join(", ")
    };
    settings.set_this_pc(format!("This PC: {} — {}", crate::netinfo::hostname(), ip_text).into());

    // The authoritative config. Kept in memory rather than re-read from disk on
    // every Save, so a first-run key that could not be written still works for
    // this session instead of vanishing on the next restart of the listener.
    let cfg_cell: Rc<RefCell<Config>> = Rc::new(RefCell::new(cfg.clone()));

    // Listener handle — lives on the UI thread (Rc, not Send; fine here).
    let listener: Rc<RefCell<Option<serve::Handle>>> = Rc::new(RefCell::new(None));

    // --- Pairing dialog ---
    let answer: AnswerSlot = Arc::new(Mutex::new(None));
    {
        let pw = pair_win.as_weak();
        let sw = settings.as_weak();
        let slot = answer.clone();
        pair_win.on_allow(move || {
            reply(&slot, true, "Allow clicked");
            if let Some(w) = pw.upgrade() {
                let _ = w.hide();
            }
            if let Some(s) = sw.upgrade() {
                s.set_status_line("Pairing approved — the Mac can connect now.".into());
            }
        });
    }
    {
        let pw = pair_win.as_weak();
        let sw = settings.as_weak();
        let slot = answer.clone();
        pair_win.on_deny(move || {
            reply(&slot, false, "Deny clicked");
            if let Some(w) = pw.upgrade() {
                let _ = w.hide();
            }
            if let Some(s) = sw.upgrade() {
                s.set_status_line("Pairing declined.".into());
            }
        });
    }
    {
        // Closing the window with the title-bar X must decline, not leave the Mac
        // hanging until the 60 s timeout.
        let slot = answer.clone();
        pair_win.window().on_close_requested(move || {
            reply(&slot, false, "dialog closed with the title-bar X");
            slint::CloseRequestResponse::HideWindow
        });
    }

    // Runs on the pairing thread: show the dialog, block for the answer.
    // `serve` guarantees only one of these at a time, so a single answer slot is
    // enough.
    let decide: serve::PairDecide = {
        let pw = pair_win.as_weak();
        let slot = answer.clone();
        Arc::new(move |req: &PairRequest| {
            plog!("decide() entered for {:?} — asking the UI thread to show the dialog", req.peer_name);
            let (tx, rx) = mpsc::channel::<bool>();
            match slot.lock() {
                Ok(mut s) => *s = Some(tx),
                Err(_) => {
                    plog!("decide(): answer slot poisoned, declining without asking");
                    return false;
                }
            }

            let (name, code) = (req.peer_name.clone(), code_display(&req.code));
            let w = pw.clone();
            // These three lines are the whole point of the trace. Between them
            // sits the gap that cannot be seen from either end: a request that
            // arrives, a closure that is queued, and a window that may or may not
            // ever reach the screen.
            if slint::invoke_from_event_loop(move || {
                match w.upgrade() {
                    Some(w) => {
                        w.set_peer_name(name.into());
                        w.set_code(code.into());
                        let shown = w.show();
                        plog!("UI thread ran the closure; show() -> {shown:?}");
                    }
                    // The event loop is alive but the window is not: nothing will
                    // ever appear and nobody can click.
                    None => plog!("UI thread ran the closure, but the dialog window is gone"),
                }
            })
            .is_err()
            {
                // UI is gone (quitting) — decline rather than hang.
                plog!("decide(): the event loop is gone, declining without asking");
                return false;
            }

            // A timeout is a decline: an unattended PC must not pair itself.
            let waited = std::time::Instant::now();
            let answered = match rx.recv_timeout(PAIR_DECISION_TIMEOUT) {
                Ok(v) => {
                    plog!("decide(): answered {v} after {:.1}s", waited.elapsed().as_secs_f32());
                    v
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    plog!(
                        "decide(): NOBODY ANSWERED in {}s — auto-declining. Either the dialog never reached the screen or it was not seen.",
                        PAIR_DECISION_TIMEOUT.as_secs()
                    );
                    false
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    plog!("decide(): the answer channel was dropped without an answer");
                    false
                }
            };

            // On the timeout path nobody hid the window or cleared the slot.
            if let Ok(mut s) = slot.lock() {
                s.take();
            }
            let w = pw.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w.upgrade() {
                    let _ = w.hide();
                }
            });
            answered
        })
    };

    // Start (stop first if running → restartable).
    let do_start: Rc<dyn Fn()> = {
        let listener = listener.clone();
        let tw = tray.as_weak();
        let sw = settings.as_weak();
        let cfg_cell = cfg_cell.clone();
        let decide = decide.clone();
        Rc::new(move || {
            let existing = listener.borrow_mut().take();
            if let Some(mut h) = existing {
                h.stop();
            }
            let Some(s) = sw.upgrade() else { return };
            // Clone out of the RefCell before any call that could re-enter it.
            let cfg = {
                let mut c = cfg_cell.borrow_mut();
                apply_ui(&mut c, &s);
                c.clone()
            };
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
                                    // Post-pairing this should not happen; it means
                                    // the Mac is holding a key from before a
                                    // "Forget paired Macs" and needs to pair again.
                                    serve::ConnStatus::HandshakeFailed => {
                                        "A Mac tried to connect with an old key — pair it again from the Mac."
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

            match serve::start(&cfg, on_conn, decide.clone()) {
                Ok(h) => {
                    *listener.borrow_mut() = Some(h);
                    if let Some(t) = tw.upgrade() {
                        t.set_active(true);
                        t.set_conn(0); // fresh listener: "Waiting for connection"
                    }
                    s.set_active(true);
                    s.set_status_line(
                        "Started — this PC is now visible to Macs on the network.".into(),
                    );
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
        let pw = pair_win.as_weak();
        let slot = answer.clone();
        Rc::new(move || {
            // Decline anything already on screen FIRST. `stop()` joins the pairing accept
            // loop but deliberately not the per-request threads, so without this a dialog
            // raised a moment ago stays up and stays answerable: clicking Allow then hands
            // the Mac a real key plus a session port that is no longer listening, and the
            // status line claims "the Mac can connect now" while 5599 refuses the
            // connection. A no-op when nothing is pending — `reply` only fires if a sender
            // is still parked in the slot.
            reply(&slot, false, "Stop");
            if let Some(w) = pw.upgrade() {
                let _ = w.hide();
            }
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
        let cfg_cell = cfg_cell.clone();
        settings.on_save(move || {
            let Some(s) = sw.upgrade() else { return };
            let cfg = {
                let mut c = cfg_cell.borrow_mut();
                apply_ui(&mut c, &s);
                c.clone()
            };
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
        let listener = listener.clone();
        let start = do_start.clone();
        let cfg_cell = cfg_cell.clone();
        settings.on_forget_devices(move || {
            let Some(s) = sw.upgrade() else { return };
            // A new key is what actually un-pairs: every Mac still holding the
            // old one now fails the handshake.
            let cfg = {
                let mut c = cfg_cell.borrow_mut();
                c.shared_secret = protocol::secure::generate_key();
                c.clone()
            };
            match cfg.save() {
                Ok(_) => {
                    s.set_status_line(
                        "Paired Macs forgotten — pair again from each Mac.".into(),
                    );
                    // The listener holds the OLD key until it is restarted, and so
                    // does the pairing listener that would hand it out.
                    if listener.borrow().is_some() {
                        start();
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

    // Start automatically at launch if a secret exists (config or env). main()
    // mints one on first run, so in practice this is always true — being visible
    // on the network is the whole point, and a PC nobody can find cannot be paired.
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
