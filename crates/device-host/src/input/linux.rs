//! X11-only native host. Wayland is rejected before opening an Xwayland display:
//! XTEST cannot provide desktop-wide control there.
use super::{keymap::xkb_name, InputBackend};
use protocol::{devices::Platform, InputEvent, MsgType};
use std::{collections::HashMap, io, ptr, sync::OnceLock};
use x11_dl::{xlib, xtest};

pub struct Linux {
    x: xlib::Xlib,
    test: xtest::Xf86vmode,
    display: *mut xlib::Display,
    keys: HashMap<[u8; 4], u32>,
}
// The display is private to this adapter; InputController serializes all calls.
// XInitThreads runs before XOpenDisplay so dropping it on another thread is valid.
unsafe impl Send for Linux {}

fn validate_session(display: bool, session: &str, wayland: bool) -> io::Result<()> {
    if session.eq_ignore_ascii_case("wayland") || wayland {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Wayland input is not implemented; select an X11 session for this adapter",
        ));
    }
    if !display {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "An active X11 DISPLAY is required",
        ));
    }
    Ok(())
}
impl Linux {
    pub fn new() -> io::Result<Self> {
        validate_session(
            std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()),
            &std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
            std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
        )?;
        let x = xlib::Xlib::open().map_err(io::Error::other)?;
        let test = xtest::Xf86vmode::open().map_err(io::Error::other)?;
        static THREADS: OnceLock<bool> = OnceLock::new();
        if !*THREADS.get_or_init(|| unsafe { (x.XInitThreads)() != 0 }) {
            return Err(io::Error::other("XInitThreads failed"));
        }
        let display = unsafe { (x.XOpenDisplay)(ptr::null()) };
        if display.is_null() {
            return Err(io::Error::other("Cannot open the authorized X11 display"));
        }
        let mut adapter = Self {
            x,
            test,
            display,
            keys: HashMap::new(),
        };
        unsafe {
            let (mut event, mut error, mut major, mut minor) = (0, 0, 0, 0);
            if (adapter.test.XTestQueryExtension)(
                display, &mut event, &mut error, &mut major, &mut minor,
            ) == 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "XTEST extension unavailable",
                ));
            }
            // XkbUseCoreKbd=0x100, XkbKeyNamesMask=1<<9. Resolve physical names
            // from the server; never assume evdev's keycode+8 numbering.
            let map = (adapter.x.XkbGetMap)(display, 0, 0x100);
            if map.is_null() {
                return Err(io::Error::other("XKB keyboard map unavailable"));
            }
            let loaded = (adapter.x.XkbGetNames)(display, 1 << 9, map) == 0;
            if loaded && !(*map).names.is_null() && !(*(*map).names).keys.is_null() {
                for code in (*map).min_key_code..=(*map).max_key_code {
                    let name = (*(*(*map).names).keys.add(code as usize))
                        .name
                        .map(|v| v as u8);
                    adapter.keys.insert(name, code.into());
                }
            }
            (adapter.x.XkbFreeKeyboard)(map, 0xff, 1);
        }
        if adapter.keys.is_empty() {
            return Err(io::Error::other("XKB physical key names unavailable"));
        }
        Ok(adapter)
    }
    fn button(&self, button: u32, down: bool) -> io::Result<()> {
        if unsafe { (self.test.XTestFakeButtonEvent)(self.display, button, i32::from(down), 0) }
            == 0
        {
            return Err(io::Error::other("XTEST rejected a mouse event"));
        }
        Ok(())
    }
}
impl Drop for Linux {
    fn drop(&mut self) {
        unsafe {
            (self.x.XCloseDisplay)(self.display);
        }
    }
}
impl InputBackend for Linux {
    fn platform(&self) -> Platform {
        Platform::Linux
    }
    fn event(&mut self, input: InputEvent) -> io::Result<()> {
        unsafe {
            match input {
                InputEvent::Key(k) => {
                    let Some(code) = xkb_name(k.hid_usage).and_then(|name| self.keys.get(name))
                    else {
                        return Ok(());
                    };
                    if (self.test.XTestFakeKeyEvent)(
                        self.display,
                        *code,
                        i32::from(k.msg != MsgType::Up),
                        0,
                    ) == 0
                    {
                        return Err(io::Error::other("XTEST rejected a key event"));
                    }
                }
                InputEvent::MouseMove { dx, dy } => {
                    let (mut root, mut child, mut x, mut y, mut wx, mut wy, mut mask) =
                        (0, 0, 0, 0, 0, 0, 0);
                    if (self.x.XQueryPointer)(
                        self.display,
                        (self.x.XDefaultRootWindow)(self.display),
                        &mut root,
                        &mut child,
                        &mut x,
                        &mut y,
                        &mut wx,
                        &mut wy,
                        &mut mask,
                    ) == 0
                    {
                        return Err(io::Error::other(
                            "Pointer is outside the default X11 screen",
                        ));
                    }
                    if (self.test.XTestFakeMotionEvent)(
                        self.display,
                        -1,
                        x.saturating_add(dx.into()),
                        y.saturating_add(dy.into()),
                        0,
                    ) == 0
                    {
                        return Err(io::Error::other("XTEST rejected pointer movement"));
                    }
                }
                InputEvent::MouseButton { button, down } => {
                    let native = match button {
                        0 => 1,
                        1 => 3,
                        2 => 2,
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "Unknown mouse button",
                            ))
                        }
                    };
                    self.button(native, down)?;
                }
                InputEvent::Scroll { dx, dy } => {
                    for (ticks, positive, negative) in [(dy, 4, 5), (dx, 7, 6)] {
                        for _ in 0..ticks.unsigned_abs() {
                            let button = if ticks > 0 { positive } else { negative };
                            self.button(button, true)?;
                            self.button(button, false)?;
                        }
                    }
                }
            }
            (self.x.XFlush)(self.display);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wayland_and_missing_displays_fail_before_any_os_input() {
        assert_eq!(
            validate_session(true, "wayland", false).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert!(validate_session(true, "", true).is_err());
        assert!(validate_session(false, "x11", false).is_err());
        assert!(validate_session(true, "x11", false).is_ok());
    }
}
