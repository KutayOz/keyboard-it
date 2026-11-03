//! Implemented endpoint functions, distinct from user-granted permissions.
use crate::devices::Platform;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub platform: Platform,
    pub view_screen: bool,
    pub capture_screen: bool,
    pub send_input: bool,
    pub receive_input: bool,
    pub clipboard: bool,
    pub decode_h264: bool,
    pub encode_h264: bool,
}

impl Capabilities {
    pub fn controller(platform: Platform) -> Self {
        Self {
            platform,
            view_screen: true,
            send_input: true,
            clipboard: true,
            decode_h264: true,
            ..Default::default()
        }
    }
    pub fn host(platform: Platform) -> Self {
        Self {
            platform,
            capture_screen: true,
            receive_input: true,
            clipboard: true,
            encode_h264: true,
            ..Default::default()
        }
    }
    pub fn encode(&self) -> [u8; 4] {
        let platform = match self.platform {
            Platform::Unknown => 0,
            Platform::Macos => 1,
            Platform::Windows => 2,
            Platform::Linux => 3,
            Platform::Android => 4,
            Platform::Ios => 5,
            Platform::Television => 6,
            Platform::SmartHome => 7,
        };
        let flags = [
            self.view_screen,
            self.capture_screen,
            self.send_input,
            self.receive_input,
            self.clipboard,
            self.decode_h264,
            self.encode_h264,
        ]
        .iter()
        .enumerate()
        .fold(0u16, |bits, (i, enabled)| bits | (u16::from(*enabled) << i));
        let f = flags.to_be_bytes();
        [1, platform, f[0], f[1]]
    }
    pub fn decode(b: &[u8]) -> io::Result<Self> {
        if b.len() != 4 || b[0] != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported capability record",
            ));
        }
        let platform = match b[1] {
            1 => Platform::Macos,
            2 => Platform::Windows,
            3 => Platform::Linux,
            4 => Platform::Android,
            5 => Platform::Ios,
            6 => Platform::Television,
            7 => Platform::SmartHome,
            _ => Platform::Unknown,
        };
        let flags = u16::from_be_bytes([b[2], b[3]]);
        let enabled = |i| flags & (1u16 << i) != 0u16;
        Ok(Self {
            platform,
            view_screen: enabled(0),
            capture_screen: enabled(1),
            send_input: enabled(2),
            receive_input: enabled(3),
            clipboard: enabled(4),
            decode_h264: enabled(5),
            encode_h264: enabled(6),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionCapabilities {
    pub screen: bool,
    pub input: bool,
    pub clipboard: bool,
}
impl SessionCapabilities {
    /// Desktop v2 without the optional exchange retains its original behavior.
    pub const LEGACY: Self = Self {
        screen: true,
        input: true,
        clipboard: true,
    };
    pub fn negotiate(controller: &Capabilities, host: &Capabilities) -> Self {
        let screen = controller.view_screen
            && host.capture_screen
            && controller.decode_h264
            && host.encode_h264;
        Self {
            screen,
            input: screen && controller.send_input && host.receive_input,
            clipboard: screen && controller.clipboard && host.clipboard,
        }
    }
}
