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
    pub shared_secret: String, // pairing key; SAME on both sides, hashed into the PSK with BLAKE2s
    #[serde(default)]
    pub peer_host: String, // the sender knows the peer IP/host; the receiver only listens
    #[serde(default)]
    pub role: Role,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    crate::DEFAULT_PORT // 5599
}

impl Default for Config {
    fn default() -> Self {
        Config {
            shared_secret: String::new(),
            peer_host: String::new(),
            role: Role::default(),
            port: default_port(),
        }
    }
}

impl Config {
    /// Is first-run setup done? The sender needs peer_host; the receiver only needs the secret.
    pub fn is_complete(&self) -> bool {
        if self.shared_secret.is_empty() {
            return false;
        }
        match self.role {
            Role::Sender => !self.peer_host.is_empty(),
            Role::Receiver => true,
        }
    }

    /// "host" or "host:port" -> normalized "host:port".
    pub fn peer_addr(&self) -> String {
        if self.peer_host.contains(':') {
            self.peer_host.clone()
        } else {
            format!("{}:{}", self.peer_host, self.port)
        }
    }

    /// Full path of config.toml. Does NOT create the directory.
    pub fn path() -> io::Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "keyboard-it", "keyboard-it").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no OS config directory (HOME/APPDATA missing?)")
        })?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load from disk. Missing file => Ok(None) (first run). Malformed TOML => Err.
    pub fn load() -> io::Result<Option<Config>> {
        let path = Self::path()?;
        match fs::read_to_string(&path) {
            Ok(text) => {
                let cfg: Config = toml::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
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
            std::process::Command::new("open").arg("-t").arg(&path).spawn()?;
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
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
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
            peer_host: "192.168.1.42".into(),
            role: Role::Sender,
            port: 5599,
        };
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
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
    }
}
