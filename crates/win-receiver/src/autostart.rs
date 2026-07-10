//! Oturum açılışı otomatik başlatma — **yükseltilmiş** Zamanlanmış Görev (`/rl highest`).
//! Enjeksiyon yükseltilmiş (admin) pencerelere de gidebilsin diye görev "highest" açılır.
//! Kur/sil tek seferlik UAC ister (`ShellExecuteW "runas"`); durum `schtasks /query` ile.

use std::io;

const TASK: &str = "keyboard-it";

/// Görev kurulu mu? (yükseltme gerektirmez)
pub fn is_enabled() -> bool {
    std::process::Command::new("schtasks")
        .args(["/query", "/tn", TASK])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Görevi kur/sil (yükseltilmiş). Zaten istenen durumdaysa hiçbir şey yapmaz.
pub fn set_enabled(on: bool) -> io::Result<()> {
    if on == is_enabled() {
        return Ok(());
    }
    let params = if on {
        let exe = std::env::current_exe()?;
        // /TR değeri = tırnaklı exe yolu (boşluklu Program Files için): /TR "\"<exe>\""
        format!(
            "/Create /TN {TASK} /TR \"\\\"{}\\\"\" /SC ONLOGON /RL HIGHEST /F",
            exe.display()
        )
    } else {
        format!("/Delete /TN {TASK} /F")
    };
    run_elevated("schtasks.exe", &params)?;

    // ShellExecuteW yalnızca LANSMANIN başarısını bildirir; schtasks asenkron
    // çalışır ve çıkış kodu okunmaz. Hemen dönersek gui'deki is_enabled()
    // yansıtmasıyla yarışırız (checkbox eski duruma geri döner) ve schtasks
    // gerçek bir hatayla çıksa bile "başarılı" deriz. Sonucu doğrula: görev
    // durumu istenen hale gelene dek kısa süre bekle, gelmezse hata döndür.
    for _ in 0..16 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if is_enabled() == on {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Other,
        "schtasks sonucu doğrulanamadı (görev durumu değişmedi)",
    ))
}

/// `schtasks`'ı UAC ile yükseltip çalıştır (gizli pencere).
fn run_elevated(file: &str, params: &str) -> io::Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let verb = HSTRING::from("runas");
    let file = HSTRING::from(file);
    let params = HSTRING::from(params);
    let h = unsafe { ShellExecuteW(None, &verb, &file, &params, PCWSTR::null(), SW_HIDE) };
    // ShellExecuteW: HINSTANCE > 32 => başarı.
    if h.0 as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "yükseltme başarısız (UAC reddedilmiş olabilir)",
        ))
    }
}
