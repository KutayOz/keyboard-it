//! Receiver credentials live separately from UI settings so a stale settings
//! save cannot restore a revoked controller. Policy changes commit to disk
//! before becoming effective. Authentication snapshots are rechecked on attach.
use crate::{config::Config, secure};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    net::{Shutdown, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const MAX_CONTROLLERS: usize = 32;
pub const LEGACY_ID: &str = "legacy-shared-key";

#[derive(Clone, Serialize, Deserialize)]
struct Credential {
    id: String,
    name: String,
    device_id: String,
    secret: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u8,
    legacy_allowed: bool,
    controllers: Vec<Credential>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerInfo {
    pub id: String,
    pub name: String,
    pub legacy: bool,
}

/// A snapshot is insufficient to authorize a session: attach() rechecks it.
#[derive(Clone)]
pub struct AuthenticationKey {
    pub id: String,
    pub key: [u8; 32],
    pub device_id: String,
}

struct LiveSession {
    token: u64,
    controller: String,
    socket: TcpStream,
}
struct State {
    revision: u64,
    policy: Policy,
    live: Vec<LiveSession>,
    next_token: u64,
}
pub struct AccessStore {
    path: Option<PathBuf>,
    // Keep one policy owner per file. A second process must not revoke only on
    // disk while the running host continues accepting its cached credentials.
    _lock: Option<fs::File>,
    legacy_secret: String,
    state: Mutex<State>,
}

pub struct SessionLease {
    store: Arc<AccessStore>,
    token: u64,
}
impl SessionLease {
    /// A revoked session cannot dispatch another OS input/clipboard action.
    /// Actions already in progress finish before revocation commits.
    pub fn dispatch<T>(&self, action: impl FnOnce() -> T) -> io::Result<T> {
        let state = self.store.state.lock().unwrap();
        if !state.live.iter().any(|s| s.token == self.token) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Controller access was revoked",
            ));
        }
        Ok(action())
    }
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        let mut s = self.store.state.lock().unwrap();
        if let Some(i) = s.live.iter().position(|v| v.token == self.token) {
            let session = s.live.swap_remove(i);
            let _ = session.socket.shutdown(Shutdown::Both);
        }
    }
}

impl AccessStore {
    pub fn open(cfg: &Config) -> io::Result<Arc<Self>> {
        let path = Config::path()?.with_extension("access.toml");
        let legacy = if cfg.shared_secret.is_empty() {
            std::env::var("KEYBOARD_IT_KEY").unwrap_or_default()
        } else {
            cfg.shared_secret.clone()
        };
        Self::open_path(&path, &legacy)
    }

    pub fn open_path(path: &Path, legacy_secret: &str) -> io::Result<Arc<Self>> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(path.with_extension("lock"))?;
        lock.try_lock().map_err(|e| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("Access policy is already in use; manage access in the running host: {e}"),
            )
        })?;
        let policy = match fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Policy {
                version: 1,
                legacy_allowed: !legacy_secret.is_empty(),
                controllers: Vec::new(),
            },
            Err(e) => return Err(e),
        };
        Self::validate(&policy)?;
        let store = Arc::new(Self {
            path: Some(path.into()),
            _lock: Some(lock),
            legacy_secret: legacy_secret.into(),
            state: Mutex::new(State {
                revision: 0,
                policy: policy.clone(),
                live: Vec::new(),
                next_token: 0,
            }),
        });
        // Establish the file before listening, including the initial legacy policy.
        store.persist(&policy)?;
        Ok(store)
    }

    /// Useful for embedded hosts/tests that deliberately do not persist settings.
    pub fn in_memory(legacy_secret: &str) -> Arc<Self> {
        Arc::new(Self {
            path: None,
            _lock: None,
            legacy_secret: legacy_secret.into(),
            state: Mutex::new(State {
                revision: 0,
                policy: Policy {
                    version: 1,
                    legacy_allowed: !legacy_secret.is_empty(),
                    controllers: Vec::new(),
                },
                live: Vec::new(),
                next_token: 0,
            }),
        })
    }

    fn validate(p: &Policy) -> io::Result<()> {
        let mut ids = std::collections::HashSet::new();
        let mut devices = std::collections::HashSet::new();
        let mut secrets = std::collections::HashSet::new();
        if p.version != 1
            || p.controllers.len() > MAX_CONTROLLERS
            || p.controllers.iter().any(|c| {
                c.id.is_empty()
                    || c.id == LEGACY_ID
                    || !ids.insert(&c.id)
                    || c.secret.is_empty()
                    || !secrets.insert(&c.secret)
                    || c.device_id.len() > 128
                    || (!c.device_id.is_empty() && !devices.insert(&c.device_id))
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid controller access policy",
            ));
        }
        Ok(())
    }

    fn persist(&self, policy: &Policy) -> io::Result<()> {
        Self::validate(policy)?;
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(policy).map_err(io::Error::other)?;
        let tmp = path.with_extension(format!("{}.tmp", secure::generate_key()));
        let result = (|| {
            use std::io::Write;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut f = options.open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    pub fn controllers(&self) -> Vec<ControllerInfo> {
        let state = self.state.lock().unwrap();
        let mut out: Vec<_> = state
            .policy
            .controllers
            .iter()
            .map(|c| ControllerInfo {
                id: c.id.clone(),
                name: c.name.clone(),
                legacy: false,
            })
            .collect();
        if state.policy.legacy_allowed && !self.legacy_secret.is_empty() {
            out.insert(
                0,
                ControllerInfo {
                    id: LEGACY_ID.into(),
                    name: "Eski ortak anahtar (önceki eşleştirmeler)".into(),
                    legacy: true,
                },
            );
        }
        out
    }

    fn keys(&self, policy: &Policy) -> Vec<AuthenticationKey> {
        let mut out: Vec<_> = policy
            .controllers
            .iter()
            .map(|c| AuthenticationKey {
                id: c.id.clone(),
                key: secure::psk_from_secret(&c.secret),
                device_id: c.device_id.clone(),
            })
            .collect();
        if policy.legacy_allowed && !self.legacy_secret.is_empty() {
            out.push(AuthenticationKey {
                id: LEGACY_ID.into(),
                key: secure::psk_from_secret(&self.legacy_secret),
                device_id: String::new(),
            });
        }
        out
    }

    pub fn authentication_keys(&self) -> Vec<AuthenticationKey> {
        self.keys(&self.state.lock().unwrap().policy)
    }

    /// Call only after the local user has approved the pairing code. Names
    /// never deduplicate controllers; an approved stable device ID may re-pair.
    pub fn grant(&self, name: &str, device_id: &str) -> io::Result<String> {
        self.grant_inner(name, device_id, None)
    }

    pub fn revision(&self) -> u64 {
        self.state.lock().unwrap().revision
    }

    /// A consent dialog opened before an access change cannot grant a new key
    /// after that change. The user must approve a fresh request instead.
    pub fn grant_if_revision(
        &self,
        name: &str,
        device_id: &str,
        revision: u64,
    ) -> io::Result<String> {
        self.grant_inner(name, device_id, Some(revision))
    }

    fn grant_inner(
        &self,
        name: &str,
        device_id: &str,
        expected_revision: Option<u64>,
    ) -> io::Result<String> {
        if device_id.len() > 128 || device_id.chars().any(char::is_control) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid controller identity",
            ));
        }
        let mut state = self.state.lock().unwrap();
        if expected_revision.is_some_and(|r| r != state.revision) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Access changed while approval was pending; pair again",
            ));
        }
        let revision = state
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Access revision exhausted"))?;
        let mut policy = state.policy.clone();
        let existing = policy
            .controllers
            .iter()
            .find(|c| !device_id.is_empty() && c.device_id == device_id)
            .map(|c| c.id.clone());
        if existing.is_none() && policy.controllers.len() >= MAX_CONTROLLERS {
            return Err(io::Error::other(
                "Controller limit reached; remove a saved controller first",
            ));
        }
        let id = existing.unwrap_or_else(secure::generate_key);
        let secret = secure::generate_key();
        policy.controllers.retain(|c| c.id != id);
        policy.controllers.push(Credential {
            id: id.clone(),
            name: name.chars().filter(|c| !c.is_control()).take(64).collect(),
            device_id: device_id.into(),
            secret: secret.clone(),
        });
        self.persist(&policy)?;
        state.policy = policy;
        state.revision = revision;
        Self::close_controller(&mut state, &id);
        Ok(secret)
    }

    fn close_controller(state: &mut State, id: &str) {
        state.live.retain(|s| {
            if s.controller != id {
                return true;
            }
            let _ = s.socket.shutdown(Shutdown::Both);
            false
        });
    }

    pub fn revoke(&self, id: &str) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let revision = state
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Access revision exhausted"))?;
        let mut policy = state.policy.clone();
        if id == LEGACY_ID && policy.legacy_allowed {
            policy.legacy_allowed = false;
        } else if policy.controllers.iter().any(|c| c.id == id) {
            policy.controllers.retain(|c| c.id != id);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Controller not found",
            ));
        }
        self.persist(&policy)?;
        state.policy = policy;
        state.revision = revision;
        Self::close_controller(&mut state, id);
        Ok(())
    }

    pub fn revoke_all(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let revision = state
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Access revision exhausted"))?;
        let policy = Policy {
            version: 1,
            legacy_allowed: false,
            controllers: Vec::new(),
        };
        self.persist(&policy)?;
        state.policy = policy;
        state.revision = revision;
        for s in state.live.drain(..) {
            let _ = s.socket.shutdown(Shutdown::Both);
        }
        Ok(())
    }

    pub fn attach(
        self: &Arc<Self>,
        key: &AuthenticationKey,
        socket: &TcpStream,
    ) -> io::Result<Arc<SessionLease>> {
        self.attach_with(key, socket, || Ok(())).map(|v| v.0)
    }

    /// Publish ownership under the same lock as revocation. A handshake that
    /// finished before revocation cannot take ownership after revoke() returns.
    pub fn attach_with<T>(
        self: &Arc<Self>,
        key: &AuthenticationKey,
        socket: &TcpStream,
        publish: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<(Arc<SessionLease>, T)> {
        let mut state = self.state.lock().unwrap();
        let allowed = self
            .keys(&state.policy)
            .iter()
            .any(|k| k.id == key.id && k.key == key.key);
        if !allowed {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Controller access was revoked",
            ));
        }
        if state.live.len() >= 128 {
            return Err(io::Error::other("Too many active controller sessions"));
        }
        let clone = socket.try_clone()?;
        state.next_token = state
            .next_token
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Session ID exhausted"))?;
        let token = state.next_token;
        let value = publish()?;
        state.live.push(LiveSession {
            token,
            controller: key.id.clone(),
            socket: clone,
        });
        Ok((
            Arc::new(SessionLease {
                store: self.clone(),
                token,
            }),
            value,
        ))
    }
}
