//! Persistent config: shared secret + peer host, in the OS-standard config dir.
//! Replaces the `KEYBOARD_IT_KEY` env var and `~/.keyboard-it-ip` (the env var
//! remains as a backward-compat fallback — see secure::psk_from_config_or_env).
//!
//! Location (ProjectDirs::from("com","keyboard-it","keyboard-it")):
//!   macOS  : ~/Library/Application Support/com.keyboard-it.keyboard-it/config.toml
//!   Windows: %APPDATA%\keyboard-it\keyboard-it\config\config.toml

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::devices::{PairedDevice, Permissions, Platform, SessionTarget};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Sender, // macOS: captures + sends
    Receiver, // Windows: receives + injects
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub devices: Vec<PairedDevice>,
    #[serde(default)]
    pub active_device_id: String,
    #[serde(default)]
    pub target_revision: u64,
    #[serde(default)]
    pub stream: crate::stream::StreamSettings,
    #[serde(default)]
    pub show_stream_stats: bool,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub peer_device_id: String,
    #[serde(default = "default_desktop_port")]
    pub desktop_port: u16,
    #[serde(default = "yes")]
    pub desktop_mode: bool,
    #[serde(default = "yes")]
    pub clipboard_enabled: bool,
    #[serde(default)]
    pub startup_configured: bool,
    #[serde(default)]
    pub shared_secret: String, // pairing key; SAME on both sides, hashed into the PSK with BLAKE2s
    #[serde(default)]
    pub peer_host: String, // the sender knows the peer IP/host; the receiver only listens
    /// Last known IP of the peer, used only if `peer_host` fails to resolve.
    /// Pairing stores the mDNS ".local" name in `peer_host` so a new DHCP lease
    /// does not break the link; this is the fallback for when mDNS resolution
    /// itself is unavailable (VPN, .local blocked) but the address still works.
    #[serde(default)]
    pub peer_ip: String,
    #[serde(default)]
    pub role: Role,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn yes() -> bool {
    true
}
fn default_desktop_port() -> u16 {
    crate::desktop::DEFAULT_PORT
}

fn default_port() -> u16 {
    crate::DEFAULT_PORT // 5599
}

impl Default for Config {
    fn default() -> Self {
        Config {
            devices: Vec::new(),
            active_device_id: String::new(),
            target_revision: 0,
            stream: Default::default(),
            show_stream_stats: false,
            device_id: String::new(),
            peer_device_id: String::new(),
            desktop_port: default_desktop_port(),
            desktop_mode: true,
            clipboard_enabled: true,
            startup_configured: false,
            shared_secret: String::new(),
            peer_host: String::new(),
            peer_ip: String::new(),
            role: Role::default(),
            port: default_port(),
        }
    }
}

static CONFIG_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Config {
    pub fn active_device(&self) -> Option<&PairedDevice> {
        self.devices.iter().find(|d| d.id == self.active_device_id)
    }

    pub fn android_selected(&self) -> bool {
        self.active_device().is_some_and(PairedDevice::is_android)
    }
    fn clear_native_target(&mut self) {
        self.shared_secret.clear();
        self.peer_host.clear();
        self.peer_ip.clear();
        self.peer_device_id.clear();
    }
    pub fn permissions(&self) -> Permissions {
        self.active_device()
            .map(|d| d.permissions.clone())
            .unwrap_or_default()
    }

    pub fn session_target(&self) -> SessionTarget {
        SessionTarget {
            transport: self
                .active_device()
                .map(|d| d.transport.clone())
                .unwrap_or_default(),
            device: self.active_device_id.clone(),
            revision: self.target_revision,
            host: self.peer_host.clone(),
            secret: self.shared_secret.clone(),
        }
    }

    /// Import old sender settings without rotating their existing pairing key.
    fn migrate_devices(&mut self) {
        if self.role != Role::Sender
            || !self.devices.is_empty()
            || !self.active_device_id.is_empty()
            || self.shared_secret.is_empty()
            || self.peer_host.is_empty()
        {
            return;
        }
        // Stable across loads before the first save, unlike a random migration ID.
        let id = format!("legacy:{}:{}", self.peer_host, self.port);
        self.devices.push(PairedDevice {
            transport: Default::default(),
            capabilities: None,
            id: id.clone(),
            name: self.peer_host.clone(),
            platform: Platform::Windows,
            permissions: Permissions::default(),
            peer_device_id: self.peer_device_id.clone(),
            shared_secret: self.shared_secret.clone(),
            host: self.peer_host.clone(),
            ip: self.peer_ip.clone(),
            port: self.port,
            desktop_port: self.desktop_port,
        });
        self.active_device_id = id;
    }

    fn sync_active_device(&mut self) {
        if self.role != Role::Sender {
            return;
        }
        if let Some(d) = self
            .devices
            .iter_mut()
            .find(|d| d.id == self.active_device_id)
        {
            if d.is_android() {
                return;
            }
            d.peer_device_id = self.peer_device_id.clone();
            d.shared_secret = self.shared_secret.clone();
            d.host = self.peer_host.clone();
            d.ip = self.peer_ip.clone();
            d.port = self.port;
            d.desktop_port = self.desktop_port;
        }
    }

    pub fn activate_device(&mut self, id: &str) -> io::Result<()> {
        let d = self
            .devices
            .iter()
            .find(|d| d.id == id)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Saved device not found"))?;
        if self.active_device_id == id {
            return Ok(());
        }
        self.sync_active_device();
        self.target_revision = self
            .target_revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Selection revision exhausted"))?;
        self.active_device_id = d.id.clone();
        if d.is_android() {
            self.clear_native_target();
            return Ok(());
        }
        self.peer_device_id = d.peer_device_id;
        self.peer_host = d.host;
        self.peer_ip = d.ip;
        self.port = d.port;
        self.desktop_port = d.desktop_port;
        self.shared_secret = d.shared_secret;
        Ok(())
    }

    /// Adds a pairing without disconnecting the device currently in use.
    /// Deduplication requires a pinned identity or the same approved credential
    /// at the same host. Discovery names/addresses alone are never sufficient.
    pub fn remember_device(&mut self, mut device: PairedDevice) -> io::Result<String> {
        if !device.valid_connection() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Incomplete pairing",
            ));
        }
        self.migrate_devices();
        self.sync_active_device();
        if device.is_android() {
            if let Some(existing) = self.devices.iter().find(|d| {
                d.android_identity()
                    .zip(device.android_identity())
                    .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
            }) {
                device.id = existing.id.clone();
                device.permissions = existing.permissions.clone();
            }
        } else if !device.peer_device_id.is_empty() {
            if let Some(existing) = self
                .devices
                .iter()
                .find(|d| !d.is_android() && d.peer_device_id == device.peer_device_id)
            {
                device.id = existing.id.clone();
                device.permissions = existing.permissions.clone();
                if device.capabilities.is_none() {
                    device.capabilities = existing.capabilities.clone();
                }
            }
        } else if let Some(existing) = self.devices.iter().find(|d| {
            !d.is_android() && d.shared_secret == device.shared_secret && d.host == device.host
        }) {
            device.id = existing.id.clone();
            device.peer_device_id = existing.peer_device_id.clone();
            device.permissions = existing.permissions.clone();
            if device.capabilities.is_none() {
                device.capabilities = existing.capabilities.clone();
            }
        }
        if device.platform == crate::devices::Platform::Unknown {
            if let Some(caps) = &device.capabilities {
                device.platform = caps.platform.clone();
            }
        }
        if device.id.is_empty() {
            device.id = crate::secure::generate_key();
        }
        if let Some(existing) = self.devices.iter().find(|d| d.id == device.id) {
            if existing.peer_device_id != device.peer_device_id
                || (existing.transport != device.transport
                    && (existing.is_android() || device.is_android())
                    && !existing
                        .android_identity()
                        .zip(device.android_identity())
                        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Device ID collision",
                ));
            }
        }
        let id = device.id.clone();
        if self.active_device_id == id
            && self.active_device().is_some_and(|d| {
                d.shared_secret == device.shared_secret
                    && d.transport == device.transport
                    && d.host == device.host
                    && d.port == device.port
                    && d.desktop_port == device.desktop_port
                    && d.permissions == device.permissions
            })
        {
            // Reapproving the same pairing may refresh the label, not the session.
            self.peer_ip = device.ip.clone();
            if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
                *d = device;
            }
            return Ok(id);
        }
        // Updating an active record must go through activation to invalidate sessions.
        let activate = self.active_device_id.is_empty() || self.active_device_id == id;
        if self.active_device_id == id {
            self.active_device_id.clear();
        }
        self.devices.retain(|d| d.id != id);
        self.devices.push(device);
        if activate {
            self.activate_device(&id)?;
        }
        Ok(id)
    }

    /// Local forgetting only. Receiver-side access is not revoked.
    pub fn forget_device(&mut self, id: &str) -> io::Result<()> {
        if !self.devices.iter().any(|d| d.id == id) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Saved device not found",
            ));
        }
        if self.active_device_id == id {
            self.target_revision = self
                .target_revision
                .checked_add(1)
                .ok_or_else(|| io::Error::other("Selection revision exhausted"))?;
            self.active_device_id.clear();
            self.shared_secret.clear();
            self.peer_host.clear();
            self.peer_ip.clear();
            self.peer_device_id.clear();
        }
        self.devices.retain(|d| d.id != id);
        // Never select another device automatically: input must have an explicit target.
        Ok(())
    }

    pub fn set_device_permissions(&mut self, id: &str, permissions: Permissions) -> io::Result<()> {
        let d = self
            .devices
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Saved device not found"))?;
        if d.permissions == permissions {
            return Ok(());
        }
        if self.active_device_id == id {
            self.target_revision = self
                .target_revision
                .checked_add(1)
                .ok_or_else(|| io::Error::other("Selection revision exhausted"))?;
        }
        d.permissions = permissions;
        Ok(())
    }

    pub fn transaction<T>(f: impl FnOnce(&mut Config) -> io::Result<T>) -> io::Result<T> {
        let _lock = CONFIG_WRITE.lock().unwrap();
        let mut cfg = Self::load()?.unwrap_or_default();
        let result = f(&mut cfg)?;
        cfg.save_inner()?;
        Ok(result)
    }
    pub fn ensure_identity(&mut self) {
        if self.device_id.is_empty() {
            self.device_id = crate::secure::generate_key();
        }
    }
    /// Reload-and-update transaction for background address/identity refreshes.
    pub fn update(f: impl FnOnce(&mut Config)) -> io::Result<()> {
        Self::transaction(|cfg| {
            f(cfg);
            Ok(())
        })
    }

    /// Is first-run setup done? The sender needs somewhere to connect to; the
    /// receiver only needs the secret. Uses the same address list the sender
    /// actually dials, so "complete" can never disagree with "connectable".
    pub fn is_complete(&self) -> bool {
        if self.role == Role::Sender && self.android_selected() {
            return true;
        }
        if self.shared_secret.is_empty() {
            return false;
        }
        match self.role {
            Role::Sender => !self.peer_addrs().is_empty(),
            Role::Receiver => true,
        }
    }

    /// "host" or "host:port" -> normalized "host:port".
    pub fn peer_addr(&self) -> String {
        Self::with_port(&self.peer_host, self.port)
    }

    /// Every address worth trying, best first: the (mDNS) host name, then the
    /// last known IP. Empty entries and duplicates are dropped, so the common
    /// case is a one-element list and the caller needs no special-casing.
    pub fn peer_addrs(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(2);
        for host in [&self.peer_host, &self.peer_ip] {
            if host.is_empty() {
                continue;
            }
            let addr = Self::with_port(host, self.port);
            if !out.contains(&addr) {
                out.push(addr);
            }
        }
        out
    }

    /// An IPv6 literal already carries colons, so only treat a trailing
    /// ":<digits>" as an explicit port.
    fn with_port(host: &str, port: u16) -> String {
        let has_port = host.rsplit_once(':').is_some_and(|(head, tail)| {
            !tail.is_empty()
                && tail.bytes().all(|b| b.is_ascii_digit())
                && (!head.contains(':') || head.ends_with(']'))
        });
        if has_port {
            host.to_string()
        } else {
            format!("{host}:{port}")
        }
    }

    /// Full path of config.toml. Does NOT create the directory.
    ///
    /// `KEYBOARD_IT_CONFIG` overrides the location outright. Both binaries
    /// resolve to the SAME path by design (one machine, one pairing), so
    /// running sender and receiver on one host for a dry run needs a way to
    /// keep them off each other's file.
    pub fn path() -> io::Result<PathBuf> {
        if let Some(p) = std::env::var_os("KEYBOARD_IT_CONFIG") {
            if !p.is_empty() {
                return Ok(PathBuf::from(p));
            }
        }
        let dirs = ProjectDirs::from("com", "keyboard-it", "keyboard-it").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no OS config directory (HOME/APPDATA missing?)",
            )
        })?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load from disk. Missing file => Ok(None) (first run). Malformed TOML => Err.
    pub fn load() -> io::Result<Option<Config>> {
        let path = Self::path()?;
        match fs::read_to_string(&path) {
            Ok(text) => {
                let cfg = Self::from_toml(&text)?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn from_toml(text: &str) -> io::Result<Self> {
        let mut cfg: Self = toml::from_str(text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let mut ids = std::collections::HashSet::new();
        for d in &cfg.devices {
            if d.id.is_empty() || !ids.insert(&d.id) || !d.valid_connection() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid saved device registry",
                ));
            }
        }
        if !cfg.active_device_id.is_empty() && cfg.active_device().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Active device is missing",
            ));
        }
        if cfg.role == Role::Sender && !cfg.devices.is_empty() && cfg.active_device_id.is_empty() {
            // An unselected registry must never fall back to stale flat settings.
            cfg.shared_secret.clear();
            cfg.peer_host.clear();
            cfg.peer_ip.clear();
            cfg.peer_device_id.clear();
        }
        cfg.migrate_devices();
        if cfg.role == Role::Sender && cfg.android_selected() {
            cfg.clear_native_target();
        }
        Ok(cfg)
    }

    /// Open config.toml in the OS default text editor (macOS/Linux); creates a
    /// default file to edit if none exists. NOTE: the Windows `win-receiver` no
    /// longer uses this (it has a Slint settings window); the mac-sender menu bar
    /// calls it.
    pub fn edit() -> io::Result<()> {
        let path = Self::path()?;
        if !path.exists() {
            Config::default().save()?;
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-t")
                .arg(&path)
                .spawn()?;
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("explorer")
                .raw_arg(format!("/select,\"{}\"", path.display()))
                .spawn()?;
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            std::process::Command::new("xdg-open").arg(&path).spawn()?;
        }
        Ok(())
    }

    /// Atomic write (write tmp, rename). Creates the config directory.
    pub fn save(&self) -> io::Result<()> {
        let _lock = CONFIG_WRITE.lock().unwrap();
        self.save_inner()
    }
    fn save_inner(&self) -> io::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut cfg = self.clone();
        cfg.migrate_devices();
        cfg.sync_active_device();
        let text = toml::to_string_pretty(&cfg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        Self::from_toml(&text)?; // reject invalid transactions before replacing the file
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, &text)?;
        // Cheap hardening: 0600 on Unix (the secret is stored in plaintext).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_toml() {
        let c = Config {
            shared_secret: "hunter2".into(),
            peer_host: "desktop-abc.local".into(),
            peer_ip: "192.168.1.42".into(),
            role: Role::Sender,
            port: 5599,
            ..Config::default()
        };
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn old_config_without_peer_ip_still_loads() {
        // Files written before pairing existed have no peer_ip key; #[serde(default)]
        // must keep them loadable rather than turning into a fatal InvalidData.
        let c: Config = toml::from_str(
            "shared_secret = \"k\"\npeer_host = \"h\"\nrole = \"sender\"\nport = 5599\n",
        )
        .unwrap();
        assert_eq!(c.peer_ip, "");
        assert!(c.desktop_mode && c.clipboard_enabled);
        assert_eq!(c.stream, crate::stream::StreamSettings::default());
        assert!(!c.show_stream_stats);
        assert_eq!(c.desktop_port, 5601);
        assert!(c.peer_device_id.is_empty());
        assert_eq!(c.peer_host, "h");
    }

    #[test]
    fn peer_addrs_prefers_host_then_ip() {
        let c = Config {
            peer_host: "pc.local".into(),
            peer_ip: "192.168.1.42".into(),
            port: 5599,
            ..Config::default()
        };
        assert_eq!(c.peer_addrs(), vec!["pc.local:5599", "192.168.1.42:5599"]);
    }

    #[test]
    fn peer_addrs_dedupes_and_skips_empty() {
        let one = Config {
            peer_host: "pc.local".into(),
            ..Config::default()
        };
        assert_eq!(one.peer_addrs(), vec!["pc.local:5599"]);

        let same = Config {
            peer_host: "192.168.1.42".into(),
            peer_ip: "192.168.1.42".into(),
            ..Config::default()
        };
        assert_eq!(same.peer_addrs(), vec!["192.168.1.42:5599"]);

        assert!(Config::default().peer_addrs().is_empty());
    }

    #[test]
    fn explicit_port_wins_but_ipv6_literal_is_not_mistaken_for_one() {
        let c = |h: &str| Config {
            peer_host: h.into(),
            port: 5599,
            ..Config::default()
        };
        assert_eq!(c("pc.local:7000").peer_addr(), "pc.local:7000");
        assert_eq!(c("pc.local").peer_addr(), "pc.local:5599");
        // A bare IPv6 literal ends in a hex group, not a port.
        assert_eq!(c("fe80::1").peer_addr(), "fe80::1:5599");
        // The bracketed form is what a user would paste, and it must round-trip.
        assert_eq!(c("[fe80::1]:7000").peer_addr(), "[fe80::1]:7000");
    }

    #[test]
    fn completeness_is_role_aware() {
        let mut c = Config::default();
        assert!(!c.is_complete()); // no secret
        c.shared_secret = "k".into();
        c.role = Role::Receiver;
        assert!(c.is_complete()); // a receiver does not need peer_host
        c.role = Role::Sender;
        assert!(!c.is_complete()); // a sender does
        c.peer_host = "host".into();
        assert!(c.is_complete());

        // The IP fallback alone is enough to connect, so it counts as complete.
        let ip_only = Config {
            shared_secret: "k".into(),
            peer_ip: "192.168.1.42".into(),
            role: Role::Sender,
            ..Config::default()
        };
        assert!(ip_only.is_complete());
    }
}
