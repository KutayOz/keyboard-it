//! Saved targets are independent of the currently selected connection. Discovery
//! is an address hint, never authority to replace a pairing or switch input.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
    Android,
    Ios,
    Television,
    SmartHome,
    #[default]
    Unknown,
}

/// Connection authority belongs to its adapter. ADB uses Android's approved
/// debugging keys; it must never inherit a native host's Noise credentials.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceTransport {
    #[default]
    Native,
    Adb {
        serial: String,
        android_id: String,
    },
}
pub fn valid_adb_serial(serial: &str) -> bool {
    !serial.is_empty()
        && serial.len() <= 256
        && !serial.starts_with('-')
        && serial
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:[]%".contains(&b))
}
pub fn valid_android_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Granted actions, kept distinct from what a remote device can implement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub screen: bool,
    pub input: bool,
    pub clipboard: bool,
}
impl Default for Permissions {
    fn default() -> Self {
        Self {
            screen: true,
            input: true,
            clipboard: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    #[serde(default)]
    pub transport: DeviceTransport,
    /// Last authenticated report, not discovery hints or local permissions.
    #[serde(default)]
    pub capabilities: Option<crate::capabilities::Capabilities>,
    /// Local immutable registry key. A discovered name/IP is not an identity.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub platform: Platform,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub peer_device_id: String,
    pub shared_secret: String,
    pub host: String,
    #[serde(default)]
    pub ip: String,
    pub port: u16,
    pub desktop_port: u16,
}

impl PairedDevice {
    pub fn android_identity(&self) -> Option<&str> {
        match &self.transport {
            DeviceTransport::Adb { android_id, .. } => Some(android_id),
            _ => None,
        }
    }
    pub fn is_android(&self) -> bool {
        matches!(self.transport, DeviceTransport::Adb { .. })
    }
    pub fn valid_connection(&self) -> bool {
        match &self.transport {
            DeviceTransport::Native => !self.shared_secret.is_empty() && !self.host.is_empty(),
            DeviceTransport::Adb { serial, android_id } => {
                self.platform == Platform::Android
                    && valid_adb_serial(serial)
                    && valid_android_id(android_id)
                    && self.shared_secret.is_empty()
                    && self.peer_device_id.is_empty()
                    && self.host.is_empty()
                    && self.ip.is_empty()
                    && self.capabilities.is_none()
            }
        }
    }
}

/// Selection revision prevents A -> B -> A from reviving an in-flight A session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTarget {
    pub transport: DeviceTransport,
    pub device: String,
    pub revision: u64,
    pub host: String,
    pub secret: String,
}
