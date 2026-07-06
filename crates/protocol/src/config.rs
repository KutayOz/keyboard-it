//! Kalıcı config: paylaşılan sır + peer host, OS-standart config dizininde.
//! `KEYBOARD_IT_KEY` env var'ı VE `~/.keyboard-it-ip`'yi değiştirir (ama env var
//! geriye-uyum için yedek olarak kalır — bkz. secure::psk_from_config_or_env).
//!
//! Konum (ProjectDirs::from("com","keyboard-it","keyboard-it")):
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
    Sender, // macOS: yakalar + gönderir
    Receiver, // Windows: alır + enjekte eder
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub shared_secret: String, // eşleşme anahtarı; iki tarafta AYNI, BLAKE2s ile PSK'ye türetilir
    #[serde(default)]
    pub peer_host: String, // sender peer IP/host bilir; receiver sadece dinler
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
    /// İlk-kurulum tamam mı? Sender peer_host ister; receiver yalnız sır ister.
    pub fn is_complete(&self) -> bool {
        if self.shared_secret.is_empty() {
            return false;
        }
        match self.role {
            Role::Sender => !self.peer_host.is_empty(),
            Role::Receiver => true,
        }
    }

    /// "host" veya "host:port" -> normalize edilmiş "host:port".
    pub fn peer_addr(&self) -> String {
        if self.peer_host.contains(':') {
            self.peer_host.clone()
        } else {
            format!("{}:{}", self.peer_host, self.port)
        }
    }

    /// config.toml'un tam yolu. Dizini OLUŞTURMAZ.
    pub fn path() -> io::Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "keyboard-it", "keyboard-it").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "OS config dizini yok (HOME/APPDATA eksik?)")
        })?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Diskten yükle. Dosya yoksa => Ok(None) (ilk çalıştırma). Bozuk TOML => Err.
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

    /// Atomik yaz (tmp yaz, rename). Config dizinini oluşturur.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, &text)?;
        // Ucuz sertleştirme: Unix'te dosyayı 0600 yap (sır düz metin).
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
        assert!(!c.is_complete()); // sır yok
        c.shared_secret = "k".into();
        c.role = Role::Receiver;
        assert!(c.is_complete()); // receiver peer_host istemez
        c.role = Role::Sender;
        assert!(!c.is_complete()); // sender peer_host ister
        c.peer_host = "host".into();
        assert!(c.is_complete());
    }
}
