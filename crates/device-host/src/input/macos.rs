use super::{keymap::hid_to_mac, InputBackend};
use core_graphics::{
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
        ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
};
use protocol::{devices::Platform, InputEvent, MsgType};
use std::io;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

pub struct Macos {
    modifiers: u8,
    buttons: u8,
}
impl Macos {
    pub fn new() -> io::Result<Self> {
        // Read permission without triggering a prompt during startup or validation.
        if !unsafe { AXIsProcessTrusted() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Enable Accessibility for keyboard-it-host in System Settings, then restart it",
            ));
        }
        Ok(Self {
            modifiers: 0,
            buttons: 0,
        })
    }
    fn source() -> io::Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::Private)
            .map_err(|_| io::Error::other("CGEventSource unavailable"))
    }
    fn flags(&self) -> CGEventFlags {
        let mut flags = CGEventFlags::empty();
        for (mask, flag) in [
            (0x11, CGEventFlags::CGEventFlagCommand), // reverse sender Cmd -> Ctrl mapping
            (0x22, CGEventFlags::CGEventFlagShift),
            (0x44, CGEventFlags::CGEventFlagAlternate),
            (0x88, CGEventFlags::CGEventFlagControl),
        ] {
            if self.modifiers & mask != 0 {
                flags |= flag;
            }
        }
        flags
    }
}
impl InputBackend for Macos {
    fn platform(&self) -> Platform {
        Platform::Macos
    }
    fn supports_text(&self) -> bool { true }
    fn text(&mut self, text: &str) -> io::Result<()> {
        for chunk in text.chars().collect::<Vec<_>>().chunks(20) {
            let text: String = chunk.iter().collect();
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(Self::source()?, 0, down)
                    .map_err(|_| io::Error::other("Could not create Unicode event"))?;
                event.set_string(&text);
                event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, protocol::INJECTED_EVENT_TAG);
                event.post(CGEventTapLocation::HID);
            }
        }
        Ok(())
    }
    fn event(&mut self, input: InputEvent) -> io::Result<()> {
        let unavailable = |_| io::Error::other("Could not create a Quartz input event");
        let event = match input {
            InputEvent::Key(k) => {
                let Some(code) = hid_to_mac(k.hid_usage) else {
                    return Ok(());
                };
                if (0xe0..=0xe7).contains(&k.hid_usage) {
                    let bit = 1u8 << (k.hid_usage - 0xe0);
                    if k.msg == MsgType::Up {
                        self.modifiers &= !bit;
                    } else {
                        self.modifiers |= bit;
                    }
                }
                let event =
                    CGEvent::new_keyboard_event(Self::source()?, code, k.msg != MsgType::Up)
                        .map_err(unavailable)?;
                event.set_integer_value_field(
                    EventField::KEYBOARD_EVENT_AUTOREPEAT,
                    i64::from(k.msg == MsgType::Repeat),
                );
                event
            }
            InputEvent::MouseMove { dx, dy } => {
                let mut point = CGEvent::new(Self::source()?)
                    .map_err(unavailable)?
                    .location();
                point.x += f64::from(dx);
                point.y += f64::from(dy);
                let (kind, button) = if self.buttons & 1 != 0 {
                    (CGEventType::LeftMouseDragged, CGMouseButton::Left)
                } else if self.buttons & 2 != 0 {
                    (CGEventType::RightMouseDragged, CGMouseButton::Right)
                } else if self.buttons & 4 != 0 {
                    (CGEventType::OtherMouseDragged, CGMouseButton::Center)
                } else {
                    (CGEventType::MouseMoved, CGMouseButton::Left)
                };
                let event = CGEvent::new_mouse_event(Self::source()?, kind, point, button)
                    .map_err(unavailable)?;
                event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, dx.into());
                event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, dy.into());
                event
            }
            InputEvent::MouseButton { button, down } => {
                let (kind, native) = match (button, down) {
                    (0, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
                    (0, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
                    (1, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
                    (1, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
                    (2, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
                    (2, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "Unknown mouse button",
                        ))
                    }
                };
                if down {
                    self.buttons |= 1 << button;
                } else {
                    self.buttons &= !(1 << button);
                }
                let point = CGEvent::new(Self::source()?)
                    .map_err(unavailable)?
                    .location();
                CGEvent::new_mouse_event(Self::source()?, kind, point, native)
                    .map_err(unavailable)?
            }
            InputEvent::Scroll { dx, dy } => CGEvent::new_scroll_event(
                Self::source()?,
                ScrollEventUnit::LINE,
                2,
                dy.into(),
                -i32::from(dx),
                0,
            )
            .map_err(unavailable)?,
        };
        event.set_flags(self.flags());
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, protocol::INJECTED_EVENT_TAG);
        event.post(CGEventTapLocation::HID);
        Ok(())
    }
}
