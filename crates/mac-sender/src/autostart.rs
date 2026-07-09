//! macOS oturum-açılışı otomatik başlatma — LaunchAgent plist + `launchctl`.
//!
//! Windows'taki `autostart.rs`nin (Zamanlanmış Görev) macOS karşılığı. Kullanıcının
//! `~/Library/LaunchAgents/` dizinine bir plist yazar; oturum açılışında `RunAtLoad`
//! ile mevcut .app ikili yolunu (std::env::current_exe) başlatır. Yükseltme (admin)
//! GEREKMEZ — LaunchAgent kullanıcı bağlamında çalışır (Erişilebilirlik izni yeter).

use std::io;
use std::path::PathBuf;

/// LaunchAgent etiketi — config bundle id ile aynı ailede.
const LABEL: &str = "com.keyboard-it.keyboard-it";

/// ~/Library/LaunchAgents/com.keyboard-it.keyboard-it.plist
fn plist_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME ayarlı değil"))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// LaunchAgent kurulu mu? (plist var mı)
pub fn is_enabled() -> bool {
    plist_path().map(|p| p.exists()).unwrap_or(false)
}

/// Oto-başlatmayı aç/kapat. İstenen durumda zaten ise no-op sayılır (idempotent).
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
        // Varsa eski kaydı boşalt, sonra yükle (idempotent). Hatalar yutulur:
        // load başarısız olsa da plist bir sonraki oturumda RunAtLoad ile çalışır.
        let _ = std::process::Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&path)
            .status();
        let _ = std::process::Command::new("launchctl")
            .args(["load", "-w"])
            .arg(&path)
            .status();
    } else if path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&path)
            .status();
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
