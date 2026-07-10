//! Autostart at logon — **elevated** Scheduled Task (`/rl highest`).
//! The task runs "highest" so injection also reaches elevated (admin) windows.
//! Install/remove asks for UAC once (`ShellExecuteW "runas"`); state via `schtasks /query`.

use std::io;

const TASK: &str = "keyboard-it";

/// Is the task installed? (no elevation required)
pub fn is_enabled() -> bool {
    std::process::Command::new("schtasks")
        .args(["/query", "/tn", TASK])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Install/remove the task (elevated). Does nothing if already in the requested state.
pub fn set_enabled(on: bool) -> io::Result<()> {
    if on == is_enabled() {
        return Ok(());
    }
    let params = if on {
        let exe = std::env::current_exe()?;
        // /TR value = quoted exe path (for Program Files with spaces): /TR "\"<exe>\""
        format!(
            "/Create /TN {TASK} /TR \"\\\"{}\\\"\" /SC ONLOGON /RL HIGHEST /F",
            exe.display()
        )
    } else {
        format!("/Delete /TN {TASK} /F")
    };
    run_elevated("schtasks.exe", &params)?;

    // ShellExecuteW only reports that the LAUNCH succeeded; schtasks runs asynchronously
    // and its exit code is never read. Returning immediately would race the is_enabled()
    // refresh in gui.rs (the checkbox would snap back) and would report success even if
    // schtasks exited with a real error. Verify the outcome: wait briefly until the task
    // state matches the request, and return an error if it never does.
    for _ in 0..16 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if is_enabled() == on {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Other,
        "could not verify schtasks result (task state did not change)",
    ))
}

/// Run `schtasks` elevated via UAC (hidden window).
fn run_elevated(file: &str, params: &str) -> io::Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let verb = HSTRING::from("runas");
    let file = HSTRING::from(file);
    let params = HSTRING::from(params);
    let h = unsafe { ShellExecuteW(None, &verb, &file, &params, PCWSTR::null(), SW_HIDE) };
    // ShellExecuteW: HINSTANCE > 32 => success.
    if h.0 as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "elevation failed (UAC may have been declined)",
        ))
    }
}
