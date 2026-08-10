//! Append-only trace of the pairing flow, written next to `config.toml`.
//!
//! Release builds set `windows_subsystem = "windows"`, so every `println!` in
//! `serve` and `gui` goes to a console that does not exist. That is fine for the
//! things a user can see in the status line, and useless for pairing: when a
//! request does not turn into a dialog on someone else's machine, there is
//! currently no way to find out which step dropped it, and the flow cannot be
//! reproduced on demand — it depends on what the desktop happened to be doing.
//!
//! So the pairing path writes its steps to a file as well. One failed attempt
//! then answers "did the request arrive", "did the closure run", "did the window
//! appear", "did anyone click" without needing a debug build on the user's PC.
//!
//! Deliberately dumb: no dependencies, no background thread, no buffering. A
//! whole pairing produces about ten lines and they are minutes apart, so the
//! cost of an open/append/close per line does not matter and the file is always
//! complete even if the process is killed.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Small enough to paste into a bug report. Past this the file is started over
/// rather than rotated: a pairing trace is only interesting fresh, and a second
/// file to reason about is worse than a lost one.
const MAX_BYTES: u64 = 128 * 1024;

/// Serializes writers. Each pairing request runs on its own thread and the UI
/// thread logs too, so without this two lines can interleave inside one line.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Next to `config.toml`, because that is the directory a user can already be
/// talked through finding.
fn log_path() -> Option<PathBuf> {
    Some(protocol::config::Config::path().ok()?.with_file_name("pairing.log"))
}

/// Local wall-clock time. Deliberately not monotonic: the point is to line the
/// trace up against "I tried at about twenty past nine".
#[cfg(windows)]
fn stamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

#[cfg(not(windows))]
fn stamp() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("epoch+{}.{:03}", d.as_secs(), d.subsec_millis())
}

/// Write one line. Never fails loudly: a diagnostic that can break pairing is
/// worse than no diagnostic.
pub fn log(args: std::fmt::Arguments<'_>) {
    let line = format!("{} {}\n", stamp(), args);
    // Still to stdout: the debug build keeps a console, and watching a live
    // reproduction there beats tailing a file.
    print!("{line}");
    let Some(path) = log_path() else { return };
    let _guard = WRITE_LOCK.lock();
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// `plog!("...")` — same shape as `println!`, but it survives a release build.
macro_rules! plog {
    ($($arg:tt)*) => { $crate::diag::log(format_args!($($arg)*)) };
}
pub(crate) use plog;
