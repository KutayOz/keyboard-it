//! win-receiver: receives encrypted key/mouse events from the Mac and injects them into
//! Windows via SendInput.
//!
//! On Windows it runs a **Slint** system tray plus a small settings window (`gui`); the
//! network loop (`serve`) runs on a background thread and can be started/stopped from the
//! GUI. Settings (key, peer IP/port) are edited in the GUI; no config file/editor is
//! opened. Outside Windows (macOS dry-run testing) there is no tray and `serve` runs
//! directly.
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
    // Do not die silently on a broken config.toml (release builds have no console, so a
    // double-click would look like nothing happened): fall back to defaults and surface
    // the error in the GUI.
    let (cfg, cfg_err) = match protocol::config::Config::load() {
        Ok(c) => (c.unwrap_or_default(), None),
        Err(e) => (
            protocol::config::Config::default(),
            Some(format!("could not read config.toml, defaults loaded: {e}")),
        ),
    };

    #[cfg(windows)]
    {
        if !single_instance() {
            return Ok(()); // already running
        }
        gui::run(cfg, cfg_err)
    }
    #[cfg(not(windows))]
    {
        if let Some(w) = &cfg_err {
            eprintln!("warning: {w}");
        }
        serve::serve(&cfg, |_| {})
    }
}

/// Single-instance guard: a named mutex. Returns `false` if one already exists.
#[cfg(windows)]
fn single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        match CreateMutexW(None, false, w!("Local\\keyboard-it-singleton")) {
            // The handle is never closed (HANDLE is not Drop) → the mutex lives as long
            // as the process.
            Ok(_h) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => true, // if the mutex cannot be created, do not block startup
        }
    }
}
