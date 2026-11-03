//! Persistent LAN connection and main-thread native viewer. Network workers never
//! activate AppKit windows, so reconnecting cannot steal focus or create Spaces.
mod metrics;
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
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
#[cfg(debug_assertions)]
pub use viewer::check_viewer;
pub use viewer::{export_metrics, handle_event, open, release_ui};

#[derive(Default)]
pub struct Snapshot {
    pub device_name: String,
    pub epoch: u64,
    pub status: String,
    pub display: Option<desktop::Display>,
    pub connected: bool,
    pub ready: bool,
    pub received_clipboard: Option<(u64, String)>,
}
pub struct Shared {
    /// Serializes publication of a session/message against target changes.
    routing: Mutex<()>,
    generation: AtomicU64,
    pub snapshot: Mutex<Snapshot>,
    pub frame: Mutex<Option<video::Frame>>,
    pub metrics: Mutex<metrics::Metrics>,
    pub stats_visible: AtomicBool,
    pub settings: Mutex<protocol::stream::StreamSettings>,
    outgoing: Mutex<Option<mpsc::SyncSender<Message>>>,
    bulk: Mutex<Option<mpsc::SyncSender<Message>>>,
    socket: Mutex<Option<TcpStream>>,
    pub visible: AtomicBool,
    pub viewer_armed: AtomicBool,
    pub allow_screen: AtomicBool,
    pub allow_input: AtomicBool,
    pub allow_clipboard: AtomicBool,
    pub paused: AtomicBool,
    pub input_enabled: AtomicBool,
    pub desktop_mode: AtomicBool,
    pub clipboard: AtomicBool,
}
impl Shared {
    /// False means overflow/disconnection; close the socket so held inputs are released.
    fn enqueue(&self, msg: Message) -> bool {
        let _route = self.routing.lock().unwrap();
        if (matches!(msg, Message::Input(_) | Message::Absolute { .. })
            && !self.allow_input.load(Ordering::SeqCst))
            || (matches!(msg, Message::Clipboard { .. })
                && !self.allow_clipboard.load(Ordering::SeqCst))
        {
            return true;
        }
        let queue = if matches!(msg, Message::Clipboard { .. }) {
            &self.bulk
        } else {
            &self.outgoing
        };
        queue
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|tx| tx.try_send(msg).is_ok())
    }
    fn new(cfg: &Config) -> Self {
        let permissions = cfg.permissions();
        Self {
            routing: Mutex::new(()),
            generation: AtomicU64::new(0),
            snapshot: Mutex::new(Snapshot {
                status: "Cihaz bağlantısı hazırlanıyor…".into(),
                ..Default::default()
            }),
            frame: Mutex::new(None),
            metrics: Mutex::new(Default::default()),
            settings: Mutex::new(cfg.stream.normalized()),
            stats_visible: AtomicBool::new(cfg.show_stream_stats),
            outgoing: Mutex::new(None),
            bulk: Mutex::new(None),
            socket: Mutex::new(None),
            visible: AtomicBool::new(false),
            viewer_armed: AtomicBool::new(false),
            allow_screen: AtomicBool::new(permissions.screen),
            allow_input: AtomicBool::new(permissions.input),
            allow_clipboard: AtomicBool::new(permissions.clipboard),
            paused: AtomicBool::new(false),
            input_enabled: AtomicBool::new(permissions.input),
            desktop_mode: AtomicBool::new(cfg.desktop_mode),
            clipboard: AtomicBool::new(cfg.clipboard_enabled && permissions.clipboard),
        }
    }

    fn current_generation(&self, generation: u64) -> io::Result<std::sync::MutexGuard<'_, ()>> {
        let guard = self.routing.lock().unwrap();
        if self.generation.load(Ordering::SeqCst) != generation {
            return Err(io::Error::other("Cihaz seçimi değişti"));
        }
        Ok(guard)
    }

    pub(crate) fn invalidate_target(&self) {
        let _route = self.routing.lock().unwrap();
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.visible.store(false, Ordering::SeqCst);
        self.viewer_armed.store(false, Ordering::SeqCst);
        self.input_enabled.store(false, Ordering::SeqCst);
        self.outgoing.lock().unwrap().take();
        self.bulk.lock().unwrap().take();
        if let Some(socket) = self.socket.lock().unwrap().take() {
            desktop::shutdown(&socket);
        }
        self.frame.lock().unwrap().take();
        let mut snap = self.snapshot.lock().unwrap();
        snap.epoch += 1;
        snap.connected = false;
        snap.ready = false;
        snap.display = None;
        snap.device_name.clear();
        snap.received_clipboard = None;
        snap.status = "Cihaz değiştiriliyor…".into();
    }
}
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
pub fn shared() -> &'static Arc<Shared> {
    SHARED.get_or_init(|| {
        Arc::new(Shared::new(
            &Config::load().ok().flatten().unwrap_or_default(),
        ))
    })
}
pub fn send(msg: Message) {
    // Never discard a key-up or ReleaseAll silently. Closing the session makes
    // the receiver release held inputs and allows a clean reconnect.
    if !shared().enqueue(msg) {
        disconnect();
    }
}

/// Called on the main thread before changing selection or access. Old queues,
/// frames and clipboard cannot be reused by the next authenticated session.
pub fn prepare_target_change() {
    crate::android::stop();
    viewer::reset_target();
    crate::capture::release_input();
    shared().invalidate_target();
    crate::capture::CONFIG_DIRTY.store(true, Ordering::SeqCst);
}
pub fn disconnect() {
    if let Some(s) = shared().socket.lock().unwrap().as_ref() {
        desktop::shutdown(s);
    }
    shared().snapshot.lock().unwrap().ready = false;
}
pub fn set_visible(visible: bool) {
    let s = shared();
    let visible =
        visible && s.allow_screen.load(Ordering::SeqCst) && s.viewer_armed.load(Ordering::SeqCst);
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
    let enabled = !s.clipboard.load(Ordering::SeqCst) && s.allow_clipboard.load(Ordering::SeqCst);
    s.clipboard.store(enabled, Ordering::SeqCst);
    let _ = Config::update(|c| c.clipboard_enabled = enabled);
    send(Message::Active {
        visible: s.visible.load(Ordering::SeqCst),
        clipboard: enabled,
    });
}
pub fn set_profile(index: usize) {
    if let Some((_, settings)) = protocol::stream::StreamSettings::PROFILES.get(index) {
        crate::android::stop();
        let _ = Config::update(|c| c.stream = *settings);
        *shared().settings.lock().unwrap() = *settings;
        shared().snapshot.lock().unwrap().ready = false;
        send(Message::ReleaseAll);
        disconnect();
        shared().frame.lock().unwrap().take();
        shared().metrics.lock().unwrap().reset();
    }
}
pub fn toggle_stats() {
    let enabled = !shared().stats_visible.load(Ordering::SeqCst);
    shared().stats_visible.store(enabled, Ordering::SeqCst);
    let _ = Config::update(|c| c.show_stream_stats = enabled);
}
pub fn toggle_connection() {
    let s = shared();
    let pause = !s.paused.load(Ordering::SeqCst);
    s.paused.store(pause, Ordering::SeqCst);
    if pause {
        crate::android::stop();
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
        crate::android::update_selection(&cfg);
        if cfg.android_selected() {
            s.allow_input.store(false, Ordering::SeqCst);
            s.allow_clipboard.store(false, Ordering::SeqCst);
            s.clipboard.store(false, Ordering::SeqCst);
            let status = crate::android::status();
            let mut snap = s.snapshot.lock().unwrap();
            snap.connected = status.running;
            snap.ready = false;
            snap.status = if status.message.is_empty() { "Android seçildi — Ekranı Aç".into() } else { status.message };
            drop(snap);
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        let permissions = cfg.permissions();
        s.allow_screen.store(permissions.screen, Ordering::SeqCst);
        s.allow_input.store(permissions.input, Ordering::SeqCst);
        s.allow_clipboard
            .store(permissions.clipboard, Ordering::SeqCst);
        s.desktop_mode.store(cfg.desktop_mode, Ordering::SeqCst);
        s.clipboard.store(
            cfg.clipboard_enabled && cfg.permissions().clipboard,
            Ordering::SeqCst,
        );
        let generation = s.generation.load(Ordering::SeqCst);
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
            let _route = s.routing.lock().unwrap();
            let current = Config::load()?.unwrap_or_default();
            if generation != s.generation.load(Ordering::SeqCst)
                || current.session_target() != cfg.session_target()
            {
                return Err(io::Error::other("Cihaz seçimi değişti"));
            }
            if !cfg.peer_device_id.is_empty() && cfg.peer_device_id != id {
                return Err(io::Error::other(
                    "Eşleşmiş cihaz kimliği değişti; yeniden eşleştirin",
                ));
            }
            // Only an authenticated peer may update addresses. Never overwrite
            // a pairing the user changed while this connection was in flight.
            let capabilities = channel.peer_capabilities.clone();
            let _ = Config::update(|c| {
                if c.session_target() == cfg.session_target() {
                    c.peer_device_id = id;
                    c.peer_ip = host;
                    c.desktop_port = port;
                    let active = c.active_device_id.clone();
                    if let Some(device) = c.devices.iter_mut().find(|d| d.id == active) {
                        if let Some(caps) = &capabilities {
                            device.platform = caps.platform.clone();
                        }
                        device.capabilities = capabilities;
                    }
                }
            });
            drop(_route);
            session(channel, &cfg, generation)
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
                Channel::connect_with_capabilities(stream, &key, &cfg.device_id,
                    &protocol::capabilities::Capabilities::controller(protocol::devices::Platform::Macos)).map_err(|e| {
                    if e.kind() == io::ErrorKind::UnexpectedEof || e.to_string().contains("version mismatch") {
                        io::Error::other(format!("Windows uygulamasının da 0.5.0 olduğundan emin olun; eski sürümde yalnız klavye/fare modunu kullanın ({e})"))
                    } else { e }
                })
            }) {
                Ok((channel, id)) => return Ok((channel, id, addr.ip().to_string(), port)),
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}
fn session(channel: Channel, cfg: &Config, generation: u64) -> io::Result<()> {
    let negotiated = channel.negotiated;
    let Channel {
        mut reader,
        mut writer,
        shutdown,
        ..
    } = channel;
    let route = shared().current_generation(generation)?;
    let permissions = cfg.permissions();
    shared()
        .allow_screen
        .store(permissions.screen && negotiated.screen, Ordering::SeqCst);
    shared()
        .allow_input
        .store(permissions.input && negotiated.input, Ordering::SeqCst);
    shared().allow_clipboard.store(
        permissions.clipboard && negotiated.clipboard,
        Ordering::SeqCst,
    );
    shared().clipboard.store(
        cfg.clipboard_enabled && permissions.clipboard && negotiated.clipboard,
        Ordering::SeqCst,
    );
    if !negotiated.input {
        shared().input_enabled.store(false, Ordering::SeqCst);
    }
    let (bulk_tx, bulk_rx) = mpsc::sync_channel::<Message>(32);
    *shared().bulk.lock().unwrap() = Some(bulk_tx);
    let (tx, rx) = mpsc::sync_channel::<Message>(256);
    *shared().outgoing.lock().unwrap() = Some(tx);
    *shared().socket.lock().unwrap() = Some(shutdown.try_clone()?);
    {
        let mut snap = shared().snapshot.lock().unwrap();
        snap.epoch += 1;
        snap.connected = true;
        snap.device_name = cfg
            .active_device()
            .map(|d| d.name.clone())
            .unwrap_or_else(|| cfg.peer_host.clone());
        snap.ready = false;
        snap.status = if !negotiated.screen {
            "Bu cihaz ekran paylaşımını desteklemiyor"
        } else if !negotiated.input {
            "Bağlandı — yalnız görüntüleme destekleniyor"
        } else {
            "Bağlandı — Ekranı Aç"
        }
        .into();
    }
    shared().metrics.lock().unwrap().reset();
    drop(route);
    let settings = cfg.stream.normalized();
    *shared().settings.lock().unwrap() = settings;
    let running = Arc::new(AtomicBool::new(true));
    let alive = running.clone();
    let target = cfg.session_target();
    let writer_thread = thread::spawn(move || {
        let mut beat = Instant::now() - Duration::from_secs(2);
        let visible = shared().visible.load(Ordering::SeqCst);
        let mut ping_id = 0u64;
        let mut result = writer.send(&Message::Configure(settings)).and_then(|_| {
            writer.send(&Message::Active {
                visible,
                clipboard: shared().clipboard.load(Ordering::SeqCst),
            })
        });
        while result.is_ok() && alive.load(Ordering::SeqCst) {
            if generation != shared().generation.load(Ordering::SeqCst)
                || shared().paused.load(Ordering::SeqCst)
                || !shared().desktop_mode.load(Ordering::SeqCst)
            {
                break;
            }
            if beat.elapsed() >= Duration::from_secs(1) {
                let current = Config::load().ok().flatten().unwrap_or_default();
                if current.session_target() != target {
                    shared().invalidate_target();
                    crate::capture::release_input();
                    break;
                }
                ping_id = ping_id.wrapping_add(1);
                shared().metrics.lock().unwrap().ping(ping_id);
                result = writer.send(&Message::Ping(ping_id));
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
            let _route = shared().current_generation(generation)?;
            match msg {
                Message::Heartbeat => {}
                Message::Pong(id) => shared().metrics.lock().unwrap().pong(id),
                Message::Stats(stats) => shared().metrics.lock().unwrap().host(stats),
                Message::Display(d) => {
                    shared().frame.lock().unwrap().take();
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
                            let started = Instant::now();
                            let frame = decoder.decode(&data).map_err(io::Error::other)?;
                            if let Some(frame) = frame {
                                let replaced =
                                    shared().frame.lock().unwrap().replace(frame).is_some();
                                shared()
                                    .metrics
                                    .lock()
                                    .unwrap()
                                    .decoded(started.elapsed().as_micros() as u64, replaced);
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
mod switching_tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn switching_closes_old_socket_and_discards_queues_clipboard_and_display() {
        let s = Shared::new(&Config::default());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        *s.socket.lock().unwrap() = Some(client);
        let (tx, _rx) = mpsc::sync_channel(4);
        *s.outgoing.lock().unwrap() = Some(tx.clone());
        *s.bulk.lock().unwrap() = Some(tx);
        s.visible.store(true, Ordering::SeqCst);
        s.viewer_armed.store(true, Ordering::SeqCst);
        {
            let mut snap = s.snapshot.lock().unwrap();
            snap.connected = true;
            snap.ready = true;
            snap.received_clipboard = Some((1, "private clipboard".into()));
        }
        s.invalidate_target();
        assert!(matches!(server.read(&mut [0]), Ok(0)));
        assert!(s.outgoing.lock().unwrap().is_none());
        assert!(s.bulk.lock().unwrap().is_none());
        assert!(!s.visible.load(Ordering::SeqCst));
        assert!(!s.viewer_armed.load(Ordering::SeqCst));
        assert!(!s.input_enabled.load(Ordering::SeqCst));
        let snap = s.snapshot.lock().unwrap();
        assert!(!snap.ready && !snap.connected);
        assert!(snap.received_clipboard.is_none() && snap.display.is_none());
    }

    #[test]
    fn delayed_sessions_cannot_publish_after_a_target_change_or_a_return_to_it() {
        let s = Shared::new(&Config::default());
        let generation = s.generation.load(Ordering::SeqCst);
        assert!(s.current_generation(generation).is_ok());
        s.invalidate_target();
        assert!(s.current_generation(generation).is_err());
        s.invalidate_target();
        assert!(s.current_generation(generation).is_err());
        assert!(s
            .current_generation(s.generation.load(Ordering::SeqCst))
            .is_ok());
    }

    #[test]
    fn permissions_block_input_and_clipboard_but_still_allow_release_all() {
        let s = Shared::new(&Config::default());
        let (tx, rx) = mpsc::sync_channel(4);
        *s.outgoing.lock().unwrap() = Some(tx.clone());
        *s.bulk.lock().unwrap() = Some(tx);
        s.allow_input.store(false, Ordering::SeqCst);
        s.allow_clipboard.store(false, Ordering::SeqCst);
        assert!(s.enqueue(Message::Absolute { x: 1, y: 2 }));
        assert!(s.enqueue(Message::Input(protocol::InputEvent::MouseMove {
            dx: 1,
            dy: 1
        })));
        assert!(s.enqueue(Message::Clipboard {
            id: 1,
            total: 1,
            offset: 0,
            data: vec![1]
        }));
        assert!(rx.try_recv().is_err());
        assert!(s.enqueue(Message::ReleaseAll));
        assert!(matches!(rx.try_recv().unwrap(), Message::ReleaseAll));
    }
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
