//! OS adapters consume the same input events; pairing and sessions are OS-independent.
use protocol::{devices::Platform, InputEvent};
use std::io;

pub trait InputBackend: Send {
    fn platform(&self) -> Platform;
    fn available(&self) -> bool {
        true
    }
    fn event(&mut self, event: InputEvent) -> io::Result<()>;
    fn supports_text(&self) -> bool { false }
    fn text(&mut self, _text: &str) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Unicode text is not implemented by this adapter"))
    }
}

pub struct DryRun;
impl InputBackend for DryRun {
    fn platform(&self) -> Platform {
        Platform::Unknown
    }
    fn available(&self) -> bool {
        false
    }
    fn event(&mut self, event: InputEvent) -> io::Result<()> {
        println!("[dry-run] {event:?}");
        Ok(())
    }
}

mod keymap;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
pub(crate) mod windows;

pub fn native() -> io::Result<Box<dyn InputBackend>> {
    #[cfg(windows)]
    {
        return Ok(Box::new(windows::Windows));
    }
    #[cfg(target_os = "macos")]
    {
        return Ok(Box::new(macos::Macos::new()?));
    }
    #[cfg(target_os = "linux")]
    {
        return Ok(Box::new(linux::Linux::new()?));
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "No native input adapter for this OS",
    ))
}

pub(crate) fn default_backend() -> Box<dyn InputBackend> {
    #[cfg(all(windows, not(test)))]
    {
        Box::new(windows::Windows)
    }
    #[cfg(any(not(windows), test))]
    {
        Box::new(DryRun)
    }
}
