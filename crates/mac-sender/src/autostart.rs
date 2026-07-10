//! macOS login autostart — LaunchAgent plist (launchctl is NOT called).
//!
//! macOS counterpart of the Windows `autostart.rs` (Scheduled Task). Writes a plist into
//! the user's `~/Library/LaunchAgents/`; at login, `RunAtLoad` launches the current .app
//! binary path (std::env::current_exe). No elevation (admin) needed — a LaunchAgent runs
//! in the user context (the Accessibility permission suffices).

use std::io;
use std::path::PathBuf;

/// LaunchAgent label — same family as the config bundle id.
const LABEL: &str = "com.keyboard-it.keyboard-it";

/// ~/Library/LaunchAgents/com.keyboard-it.keyboard-it.plist
fn plist_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// Is the LaunchAgent installed? (does the plist exist)
pub fn is_enabled() -> bool {
    plist_path().map(|p| p.exists()).unwrap_or(false)
}

/// Enable/disable autostart. No-op when already in the requested state (idempotent).
pub fn set_enabled(on: bool) -> io::Result<()> {
    let path = plist_path()?;
    if on {
        let exe = std::env::current_exe()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
</dict>
</plist>
"#,
            exe = exe.display()
        );
        std::fs::write(&path, plist)?;
        // launchctl load/unload is NOT called: when the app was started from the
        // LaunchAgent it IS that launchd job — unload would SIGTERM the running process
        // itself. RunAtLoad is only evaluated at login; writing the plist is enough and
        // the running instance is untouched.
    } else if path.exists() {
        // Only delete the plist — no unload: if this process is its own job it would die
        // instantly, skipping the applicationWillTerminate cleanup (cursor re-association).
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
