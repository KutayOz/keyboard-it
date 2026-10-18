//! Persistent LAN connection and main-thread native viewer. Network workers never
//! activate AppKit windows, so reconnecting cannot steal focus or create Spaces.
mod video;
mod viewer;
use protocol::{
    config::Config,
    desktop::{self, Assembly, Channel, Message},
};
use std::{
    io,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
pub use viewer::{handle_event, open, release_ui};

#[derive(Default)]
pub struct Snapshot {
    pub epoch: u64,
    pub status: String,
    pub display: Option<desktop::Display>,
    pub connected: bool,
    pub ready: bool,
    pub received_clipboard: Option<(u64, String)>,
}
pub struct Shared {
    pub snapshot: Mutex<Snapshot>,
    pub frame: Mutex<Option<video::Frame>>,
    outgoing: Mutex<Option<mpsc::SyncSender<Message>>>,
    bulk: Mutex<Option<mpsc::SyncSender<Message>>>,
    socket: Mutex<Option<TcpStream>>,
    pub visible: AtomicBool,
    pub paused: AtomicBool,
    pub input_enabled: AtomicBool,
    pub desktop_mode: AtomicBool,
    pub clipboard: AtomicBool,
}
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
pub fn shared() -> &'static Arc<Shared> {
    SHARED.get_or_init(|| {
        Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                status: "Windows bağlantısı hazırlanıyor…".into(),
                ..Default::default()
            }),
            frame: Mutex::new(None),
            outgoing: Mutex::new(None),
            bulk: Mutex::new(None),
            socket: Mutex::new(None),
            visible: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            input_enabled: AtomicBool::new(true),
            desktop_mode: AtomicBool::new(true),
            clipboard: AtomicBool::new(true),
        })
    })
}
pub fn send(msg: Message) {
    let queue = if matches!(msg, Message::Clipboard { .. }) {
        &shared().bulk
    } else {
        &shared().outgoing
    };
    let failed = queue
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|tx| tx.try_send(msg).is_err());
    // Never discard a key-up or ReleaseAll silently. Closing the session makes
    // the receiver release held inputs and allows a clean reconnect.
    if failed {
        disconnect();
    }
}
pub fn disconnect() {
    if let Some(s) = shared().socket.lock().unwrap().as_ref() {
        desktop::shutdown(s);
    }
    shared().snapshot.lock().unwrap().ready = false;
}
pub fn set_visible(visible: bool) {
    let s = shared();
    if s.visible.swap(visible, Ordering::SeqCst) != visible {
        if !visible {
            send(Message::ReleaseAll);
            s.frame.lock().unwrap().take();
            s.snapshot.lock().unwrap().ready = false;
        }
        send(Message::Active {
            visible,
            clipboard: visible && s.clipboard.load(Ordering::SeqCst),
        });
    }
}
pub fn toggle_mode() {
    let s = shared();
    let enabled = !s.desktop_mode.load(Ordering::SeqCst);
    s.desktop_mode.store(enabled, Ordering::SeqCst);
    set_visible(false);
    disconnect();
    let _ = Config::update(|c| c.desktop_mode = enabled);
    crate::capture::CONFIG_DIRTY.store(true, Ordering::SeqCst);
}
pub fn toggle_clipboard() {
    let s = shared();
    let enabled = !s.clipboard.load(Ordering::SeqCst);
    s.clipboard.store(enabled, Ordering::SeqCst);
    let _ = Config::update(|c| c.clipboard_enabled = enabled);
    send(Message::Active {
        visible: s.visible.load(Ordering::SeqCst),
        clipboard: enabled,
    });
}
pub fn toggle_connection() {
    let s = shared();
    let pause = !s.paused.load(Ordering::SeqCst);
    s.paused.store(pause, Ordering::SeqCst);
    if pause {
        set_visible(false);
        disconnect();
    }
    crate::capture::CONFIG_DIRTY.store(true, Ordering::SeqCst);
}
pub fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(|| loop {
        let s = shared();
        let cfg = Config::load().ok().flatten().unwrap_or_default();
        s.desktop_mode.store(cfg.desktop_mode, Ordering::SeqCst);
        s.clipboard.store(cfg.clipboard_enabled, Ordering::SeqCst);
        let configured =
            !cfg.peer_addrs().is_empty() && protocol::secure::psk_from_config_or_env(&cfg).is_ok();
        if !configured || !cfg.desktop_mode || s.paused.load(Ordering::SeqCst) {
            s.snapshot.lock().unwrap().status = if s.paused.load(Ordering::SeqCst) {
                "Bağlantı duraklatıldı"
            } else if !cfg.desktop_mode {
                "Yalnız klavye/fare modu"
            } else {
                "Önce Ayarlar’dan Windows PC’nizi eşleştirin"
            }
            .into();
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        crate::discovery::start();
        let result = connect(&cfg).and_then(|(channel, id, host, port)| {
            if !cfg.peer_device_id.is_empty() && cfg.peer_device_id != id {
                return Err(io::Error::other(
                    "Eşleşmiş cihaz kimliği değişti; yeniden eşleştirin",
                ));
            }
            // Only an authenticated peer may update addresses. Never overwrite
            // a pairing the user changed while this connection was in flight.
            let _ = Config::update(|c| {
                if c.shared_secret == cfg.shared_secret && c.peer_host == cfg.peer_host {
                    c.peer_device_id = id;
                    c.peer_ip = host;
                    c.desktop_port = port;
                }
            });
            session(channel, &cfg)
        });
        {
            let mut snap = s.snapshot.lock().unwrap();
            snap.connected = false;
            snap.ready = false;
            snap.received_clipboard = None;
            snap.status = result
                .err()
                .map(|e| format!("Yeniden bağlanılıyor — {e}"))
                .unwrap_or_else(|| "Yeniden bağlanılıyor…".into());
        }
        s.outgoing.lock().unwrap().take();
        s.bulk.lock().unwrap().take();
        s.socket.lock().unwrap().take();
        s.frame.lock().unwrap().take();
        thread::sleep(Duration::from_millis(500));
    });
}
fn connect(cfg: &Config) -> io::Result<(Channel, String, String, u16)> {
    let key = protocol::secure::psk_from_config_or_env(cfg)?;
    let candidates = connection_candidates(cfg, &crate::discovery::peers())?;
    let mut last = io::Error::other("PC aynı ağda görünmüyor");
    for (host, port) in candidates {
        if shared().paused.load(Ordering::SeqCst) || !shared().desktop_mode.load(Ordering::SeqCst) {
            return Err(io::Error::other("Bağlantı duraklatıldı"));
        }
        let addr = format!("{}:{}", host.trim_end_matches('.'), port);
        // Discovery yields literal IPv4 addresses first. Stored names are only
        // fallback candidates, with per-address connection deadlines.
        let addresses: Vec<SocketAddr> = match addr.to_socket_addrs() {
            Ok(a) => a.filter(|v| v.is_ipv4()).collect(),
            Err(e) => {
                last = e;
                continue;
            }
        };
        for addr in addresses {
            match TcpStream::connect_timeout(&addr, Duration::from_millis(750)).and_then(|stream| {
                *shared().socket.lock().unwrap() = Some(stream.try_clone()?);
                Channel::connect(stream, &key, &cfg.device_id, true)
            }) {
                Ok((channel, id)) => return Ok((channel, id, addr.ip().to_string(), port)),
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}
fn session(channel: Channel, cfg: &Config) -> io::Result<()> {
    let Channel {
        mut reader,
        mut writer,
        shutdown,
    } = channel;
    let (bulk_tx, bulk_rx) = mpsc::sync_channel::<Message>(32);
    *shared().bulk.lock().unwrap() = Some(bulk_tx);
    let (tx, rx) = mpsc::sync_channel::<Message>(256);
    *shared().outgoing.lock().unwrap() = Some(tx);
    *shared().socket.lock().unwrap() = Some(shutdown.try_clone()?);
    {
        let mut snap = shared().snapshot.lock().unwrap();
        snap.epoch += 1;
        snap.connected = true;
        snap.ready = false;
        snap.status = "Bağlandı — Windows’u Aç".into();
    }
    let running = Arc::new(AtomicBool::new(true));
    let alive = running.clone();
    let secret = cfg.shared_secret.clone();
    let host = cfg.peer_host.clone();
    let writer_thread = thread::spawn(move || {
        let mut beat = Instant::now() - Duration::from_secs(2);
        let visible = shared().visible.load(Ordering::SeqCst);
        let mut result = writer.send(&Message::Active {
            visible,
            clipboard: shared().clipboard.load(Ordering::SeqCst),
        });
        while result.is_ok() && alive.load(Ordering::SeqCst) {
            if shared().paused.load(Ordering::SeqCst)
                || !shared().desktop_mode.load(Ordering::SeqCst)
            {
                break;
            }
            if beat.elapsed() >= Duration::from_secs(1) {
                let current = Config::load().ok().flatten().unwrap_or_default();
                if current.shared_secret != secret || current.peer_host != host {
                    break;
                }
                result = writer.send(&Message::Heartbeat);
                beat = Instant::now();
                if result.is_err() {
                    break;
                }
            }
            // Input and ReleaseAll preempt bulk clipboard fragments.
            let next = rx.try_recv().ok().or_else(|| bulk_rx.try_recv().ok());
            if let Some(msg) = next {
                result = writer.send(&msg);
            } else {
                match rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(msg) => result = writer.send(&msg),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
            }
        }
        desktop::shutdown(&shutdown);
        result
    });
    let result = (|| -> io::Result<()> {
        let mut video_assembly = Assembly::default();
        let mut clip_assembly = Assembly::default();
        let mut decoder = video::Decoder::default();
        loop {
            let msg = reader.recv()?;
            match msg {
                Message::Heartbeat => {}
                Message::Display(d) => {
                    decoder = video::Decoder::default();
                    video_assembly.clear();
                    let mut snap = shared().snapshot.lock().unwrap();
                    snap.display = Some(d);
                    snap.ready = false;
                }
                Message::Status(text) => {
                    if text.contains("durakladı") || text.contains("kullanılamıyor") {
                        shared().snapshot.lock().unwrap().ready = false;
                    }
                    shared().snapshot.lock().unwrap().status = text;
                }
                Message::Video {
                    id,
                    total,
                    offset,
                    data,
                    ..
                } => {
                    if let Some(data) =
                        video_assembly.push(id, total, offset, &data, desktop::MAX_VIDEO)?
                    {
                        if shared().visible.load(Ordering::SeqCst) {
                            let frame = decoder.decode(&data).map_err(io::Error::other)?;
                            if let Some(frame) = frame {
                                *shared().frame.lock().unwrap() = Some(frame);
                            }
                        }
                    }
                }
                Message::Clipboard {
                    id,
                    total,
                    offset,
                    data,
                } => {
                    if shared().visible.load(Ordering::SeqCst)
                        && shared().clipboard.load(Ordering::SeqCst)
                    {
                        if let Some(bytes) =
                            clip_assembly.push(id, total, offset, &data, desktop::MAX_CLIPBOARD)?
                        {
                            let text = String::from_utf8(bytes).map_err(io::Error::other)?;
                            shared().snapshot.lock().unwrap().received_clipboard = Some((id, text));
                        }
                    } else {
                        clip_assembly.clear();
                    }
                }
                _ => return Err(io::Error::other("Beklenmeyen masaüstü mesajı")),
            }
        }
    })();
    running.store(false, Ordering::SeqCst);
    disconnect();
    let _ = writer_thread.join();
    result
}

fn host_name(host: &str) -> String {
    if let Ok(addr) = host.parse::<SocketAddr>() {
        return addr.ip().to_string();
    }
    let host = host
        .rsplit_once(':')
        .filter(|(head, port)| !head.contains(':') && port.parse::<u16>().is_ok())
        .map(|v| v.0)
        .unwrap_or(host);
    host.trim_end_matches('.').into()
}
fn connection_candidates(
    cfg: &Config,
    peers: &[crate::discovery::DiscoveredPeer],
) -> io::Result<Vec<(String, u16)>> {
    let mut candidates = vec![];
    for p in peers.iter().filter(|p| {
        (!cfg.peer_device_id.is_empty() && p.device_id == cfg.peer_device_id)
            || (cfg.peer_device_id.is_empty()
                && (p.hostname.trim_end_matches('.') == host_name(&cfg.peer_host)
                    || p.host == cfg.peer_ip
                    || p.host == host_name(&cfg.peer_host)))
    }) {
        if let Some(port) = p.desktop_port {
            candidates.push((p.host.clone(), port));
        } else {
            return Err(io::Error::other(
                "Windows uygulamasını güncelleyin; yalnız klavye/fare modu kullanılabilir",
            ));
        }
    }
    for host in [&cfg.peer_ip, &cfg.peer_host] {
        let host = host_name(host);
        if !host.is_empty() && !candidates.contains(&(host.clone(), cfg.desktop_port)) {
            candidates.push((host, cfg.desktop_port));
        }
    }
    Ok(candidates)
}
#[cfg(test)]
mod rediscovery_tests {
    use super::*;
    fn peer(id: &str, host: &str) -> crate::discovery::DiscoveredPeer {
        crate::discovery::DiscoveredPeer {
            fullname: "pc._keyboard-it._tcp.local.".into(),
            name: "PC".into(),
            host: host.into(),
            hostname: "pc.local".into(),
            port: 5599,
            pair_port: Some(5600),
            desktop_port: Some(6000),
            device_id: id.into(),
        }
    }
    #[test]
    fn dhcp_change_uses_identity_before_stale_address() {
        let cfg = Config {
            peer_device_id: "trusted-id".into(),
            peer_host: "old-name.local".into(),
            peer_ip: "192.168.0.2".into(),
            ..Default::default()
        };
        let found = connection_candidates(
            &cfg,
            &[
                peer("other", "192.168.0.99"),
                peer("trusted-id", "192.168.0.8"),
            ],
        )
        .unwrap();
        assert_eq!(found[0], ("192.168.0.8".into(), 6000));
        assert!(!found.iter().any(|p| p.0 == "192.168.0.99"));
    }
    #[test]
    fn old_host_with_explicit_input_port_can_migrate_without_pairing() {
        let cfg = Config {
            peer_host: "pc.local:5599".into(),
            ..Default::default()
        };
        assert_eq!(
            connection_candidates(&cfg, &[peer("new-id", "192.168.0.8")]).unwrap()[0],
            ("192.168.0.8".into(), 6000)
        );
        assert_eq!(
            connection_candidates(&cfg, &[]).unwrap(),
            vec![("pc.local".into(), 5601)]
        );
    }
}
