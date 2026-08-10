//! Autostart at logon — **elevated** Scheduled Task (`/rl highest`).
//! The task runs "highest" so injection also reaches elevated (admin) windows.
//! Install/remove asks for UAC once (`ShellExecuteW "runas"`); state via `schtasks /query`.
//! Both are BLOCKING and must not be called from the UI thread — see `set_enabled`.

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
    // Timed, because the budget below only makes sense if we know what it is
    // waiting for: whether ShellExecuteW returns as soon as the elevation is
    // requested or only once the human has answered the consent dialog decides
    // whether the wait covers a person or just a process.
    let t0 = std::time::Instant::now();
    let elevation = run_elevated("schtasks.exe", &params);
    crate::diag::plog!(
        "autostart: elevation request returned after {:.1}s -> {:?}",
        t0.elapsed().as_secs_f32(),
        elevation.as_ref().err().map(|e| e.to_string())
    );
    elevation?;

    // ShellExecuteW only reports that the LAUNCH succeeded; schtasks runs asynchronously
    // and its exit code is never read. Returning immediately would race the is_enabled()
    // refresh in gui.rs (the checkbox would snap back) and would report success even if
    // schtasks exited with a real error. Verify the outcome: wait until the task state
    // matches the request, and return an error if it never does.
    //
    // The budget is sized for a PERSON, not a process. It used to be 4 s, which is
    // ample for schtasks and nowhere near enough for someone reading a UAC prompt
    // before approving it: a toggle that had in fact succeeded reported "could not
    // verify schtasks result", the checkbox snapped back, and the user re-ticked
    // something that already worked. 120 s is what Windows itself allows the
    // consent dialog before auto-declining, so past it there is genuinely nothing
    // left to wait for. Affordable only because gui.rs now runs this on a worker
    // thread — on the UI thread it would be a two-minute freeze.
    let deadline = std::time::Instant::now() + VERIFY_BUDGET;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if is_enabled() == on {
            crate::diag::plog!(
                "autostart: task state confirmed {:.1}s after the elevation returned",
                t0.elapsed().as_secs_f32()
            );
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Other,
        "Windows did not confirm the change (the permission prompt may have been dismissed)",
    ))
}

/// How long to wait for the task to appear or disappear after elevation — see
/// the reasoning in [`set_enabled`]. Matches UAC's own auto-decline.
const VERIFY_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

/// Run `schtasks` elevated via UAC (hidden window).
fn run_elevated(file: &str, params: &str) -> io::Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    // The shell APIs want an initialized apartment. This used to run on the UI
    // thread, where winit had already done it; now that gui.rs calls this from a
    // worker, do it here. Ignore the result: RPC_E_CHANGED_MODE just means this
    // thread was already initialized differently, which is not our problem to fix.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

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
