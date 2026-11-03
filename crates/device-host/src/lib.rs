//! Platform-independent authenticated host runtime, shared by the Windows tray
//! and the console host. Native input adapters do not own pairing or transport.
pub mod console;
pub mod desktop;
pub mod diag;
pub mod inject;
pub mod input;
#[cfg(windows)]
pub mod netinfo;
#[cfg(any(windows, test))]
mod scancode;
pub mod serve;
