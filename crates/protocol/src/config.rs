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

fn default_port() -> u16 {
    crate::DEFAULT_PORT // 5599
}

impl Default for Config {
    fn default() -> Self {
        Config {
            shared_secret: String::new(),
            peer_host: String::new(),
            peer_ip: String::new(),
            role: Role::default(),
            port: default_port(),
        }
    }
}

impl Config {
    /// Is first-run setup done? The sender needs somewhere to connect to; the
    /// receiver only needs the secret. Uses the same address list the sender
    /// actually dials, so "complete" can never disagree with "connectable".
    pub fn is_complete(&self) -> bool {
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
        let has_port = host
            .rsplit_once(':')
            .is_some_and(|(head, tail)| {
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
            peer_host: "desktop-abc.local".into(),
            peer_ip: "192.168.1.42".into(),
            role: Role::Sender,
            port: 5599,
        };
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn old_config_without_peer_ip_still_loads() {
        // Files written before pairing existed have no peer_ip key; #[serde(default)]
        // must keep them loadable rather than turning into a fatal InvalidData.
        let c: Config =
            toml::from_str("shared_secret = \"k\"\npeer_host = \"h\"\nrole = \"sender\"\nport = 5599\n")
                .unwrap();
        assert_eq!(c.peer_ip, "");
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
        let one = Config { peer_host: "pc.local".into(), ..Config::default() };
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
        let c = |h: &str| Config { peer_host: h.into(), port: 5599, ..Config::default() };
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
