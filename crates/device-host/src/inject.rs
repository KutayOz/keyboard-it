//! Session ownership and held-input cleanup shared by all native adapters.
use crate::input::{self, InputBackend};
use protocol::{InputEvent, KeyEvent, MsgType};
use std::io;

/// Shared between legacy and desktop channels. Injection and release happen under
/// one lock so a superseded connection can never release the new owner's keys.
struct Controller {
    backend: Box<dyn InputBackend>,
    generation: u64,
    live: bool,
    keys: std::collections::HashSet<u16>,
    buttons: std::collections::HashSet<u8>,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            backend: input::default_backend(),
            generation: 0,
            live: false,
            keys: Default::default(),
            buttons: Default::default(),
        }
    }
}
/// Explicitly enable OS injection before starting listeners. Tests and non-Windows
/// dry runs retain a backend that never posts physical input.
pub fn enable_native() -> io::Result<()> {
    set_backend(input::native()?)
}
pub fn set_backend(backend: Box<dyn InputBackend>) -> io::Result<()> {
    let mut c = controller().lock().unwrap();
    if c.live {
        return Err(io::Error::other(
            "Stop the host before changing its input adapter",
        ));
    }
    c.backend = backend;
    Ok(())
}
pub fn capabilities() -> protocol::capabilities::Capabilities {
    let c = controller().lock().unwrap();
    protocol::capabilities::Capabilities {
        platform: c.backend.platform(),
        receive_input: c.backend.available(),
        ..Default::default()
    }
}
pub fn disconnect() {
    let mut c = controller().lock().unwrap();
    c.release();
    c.live = false;
}
#[cfg(test)]
pub static TEST_CONTROLLER: std::sync::Mutex<()> = std::sync::Mutex::new(());
static CONTROLLER: std::sync::OnceLock<std::sync::Mutex<Controller>> = std::sync::OnceLock::new();
fn controller() -> &'static std::sync::Mutex<Controller> {
    CONTROLLER.get_or_init(Default::default)
}
impl Controller {
    fn release(&mut self) {
        for hid in self.keys.drain() {
            let _ = self.backend.event(InputEvent::Key(KeyEvent {
                msg: MsgType::Up,
                hid_usage: hid,
                modifiers: 0,
            }));
        }
        for button in self.buttons.drain() {
            let _ = self.backend.event(InputEvent::MouseButton {
                button,
                down: false,
            });
        }
    }
}
pub fn acquire() -> u64 {
    let mut c = controller().lock().unwrap();
    c.release();
    c.generation += 1;
    c.live = true;
    c.generation
}
pub fn release(owner: u64) {
    let mut c = controller().lock().unwrap();
    if c.generation == owner {
        c.release();
    }
}
pub fn owned(owner: u64) -> bool {
    let c = controller().lock().unwrap();
    c.live && c.generation == owner
}
pub fn connected() -> bool {
    controller().lock().unwrap().live
}
pub fn retire(owner: u64) {
    let mut c = controller().lock().unwrap();
    if c.generation == owner {
        c.release();
        c.live = false;
    }
}
pub fn event(owner: u64, ev: InputEvent) -> bool {
    let mut c = controller().lock().unwrap();
    if !c.live || c.generation != owner {
        return false;
    }
    // Track presses before posting: a partially accepted OS call still needs a
    // release attempt. Remove releases only after the backend accepts them.
    match ev {
        InputEvent::Key(k) if k.msg != MsgType::Up => {
            c.keys.insert(k.hid_usage);
        }
        InputEvent::MouseButton { button, down: true } => {
            c.buttons.insert(button);
        }
        _ => {}
    }
    if let Err(e) = c.backend.event(ev) {
        eprintln!("Input adapter failed: {e}");
        return false;
    }
    match ev {
        InputEvent::Key(k) if k.msg == MsgType::Up => {
            c.keys.remove(&k.hid_usage);
        }
        InputEvent::MouseButton {
            button,
            down: false,
        } => {
            c.buttons.remove(&button);
        }
        _ => {}
    }
    true
}
#[cfg(windows)]
pub fn absolute(owner: u64, x: u16, y: u16, d: &protocol::desktop::Display) {
    if cfg!(test) {
        return;
    }
    let c = controller().lock().unwrap();
    if !c.live || c.generation != owner || !c.backend.available() {
        return;
    }
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };
        let px = d.left as i64 + x as i64 * (d.source_width - 1) as i64 / 65535;
        let py = d.top as i64 + y as i64 * (d.source_height - 1) as i64 / 65535;
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN) as i64;
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN) as i64;
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2) as i64;
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2) as i64;
        crate::input::windows::send_mouse(
            ((px - vx) * 65535 / (vw - 1)).clamp(0, 65535) as i32,
            ((py - vy) * 65535 / (vh - 1)).clamp(0, 65535) as i32,
            0,
            MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
        );
    }
    #[cfg(not(windows))]
    let _ = (x, y, d);
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    struct Recording(std::sync::Arc<std::sync::Mutex<Vec<InputEvent>>>);
    impl InputBackend for Recording {
        fn platform(&self) -> protocol::devices::Platform {
            protocol::devices::Platform::Unknown
        }
        fn event(&mut self, event: InputEvent) -> io::Result<()> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }
    #[test]
    fn replacement_and_stop_release_the_old_adapters_held_keys_and_buttons() {
        let _guard = TEST_CONTROLLER.lock().unwrap();
        disconnect();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let original = std::mem::replace(
            &mut controller().lock().unwrap().backend,
            Box::new(Recording(events.clone())),
        );
        let old = acquire();
        assert!(event(
            old,
            InputEvent::Key(KeyEvent {
                msg: MsgType::Down,
                hid_usage: 4,
                modifiers: 0
            })
        ));
        assert!(event(
            old,
            InputEvent::MouseButton {
                button: 0,
                down: true
            }
        ));
        let new = acquire();
        assert!(events.lock().unwrap().contains(&InputEvent::Key(KeyEvent {
            msg: MsgType::Up,
            hid_usage: 4,
            modifiers: 0
        })));
        assert!(events.lock().unwrap().contains(&InputEvent::MouseButton {
            button: 0,
            down: false
        }));
        let before = events.lock().unwrap().len();
        retire(old);
        assert_eq!(events.lock().unwrap().len(), before);
        assert!(owned(new));
        assert!(event(
            new,
            InputEvent::Key(KeyEvent {
                msg: MsgType::Down,
                hid_usage: 5,
                modifiers: 0
            })
        ));
        disconnect();
        assert!(!connected());
        assert_eq!(
            events.lock().unwrap().last(),
            Some(&InputEvent::Key(KeyEvent {
                msg: MsgType::Up,
                hid_usage: 5,
                modifiers: 0
            }))
        );
        controller().lock().unwrap().backend = original;
    }
    #[test]
    fn old_disconnect_cannot_release_or_inject_into_new_owner() {
        let _guard = TEST_CONTROLLER.lock().unwrap();
        let old = acquire();
        let new = acquire();
        assert!(!event(old, InputEvent::MouseMove { dx: 1, dy: 1 }));
        release(old);
        assert!(owned(new));
        assert!(event(new, InputEvent::MouseMove { dx: 1, dy: 1 }));
        release(new);
    }
}
