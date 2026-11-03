//! Controller transport shared with Android. Discovery is never an authority;
//! only locally verified pairing and a pinned authenticated Hello establish trust.
use protocol::{
    capabilities::Capabilities,
    desktop::{self, Channel, Message},
    devices::Platform,
    pairing, secure, InputEvent,
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{IpAddr, Shutdown, SocketAddr, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub desktop_port: u16,
    pub device_id: String,
    pub secret: String,
}
impl Target {
    fn validate(&self) -> io::Result<()> {
        address(&self.host, self.desktop_port)?;
        if self.device_id.is_empty()
            || self.device_id.len() > 128
            || self.secret.is_empty()
            || self.secret.len() > 255
        {
            return Err(invalid("Missing or invalid saved pairing"));
        }
        Ok(())
    }
}
fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn address(host: &str, port: u16) -> io::Result<SocketAddr> {
    let ip: IpAddr = host
        .parse()
        .map_err(|_| invalid("Use an explicit IPv4 or IPv6 address"))?;
    if port == 0 || ip.is_unspecified() || ip.is_multicast() {
        return Err(invalid("Invalid peer address"));
    }
    Ok(SocketAddr::new(ip, port))
}
fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let s = TcpStream::connect_timeout(&address(host, port)?, Duration::from_secs(4))?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(s)
}

pub enum PairEvent {
    Fingerprint(String),
    Paired(Target),
    Failed(String),
}
pub struct Pairing {
    socket: TcpStream,
    events: Mutex<Receiver<PairEvent>>,
    cancelled: Arc<AtomicBool>,
}
impl Pairing {
    pub fn start(
        host: &str,
        port: u16,
        desktop_port: u16,
        name: &str,
        identity: &str,
    ) -> io::Result<Self> {
        address(host, desktop_port)?;
        let mut socket = connect(host, port)?;
        let control = socket.try_clone()?;
        let wait = socket.try_clone()?;
        let (tx, rx) = mpsc::sync_channel(3);
        let host = host.to_owned();
        let name = name.to_owned();
        let identity = identity.to_owned();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        thread::spawn(move || {
            let result =
                pairing::pair_initiator_with_identity(&mut socket, &name, &identity, |code| {
                    let _ = wait.set_read_timeout(Some(pairing_wait()));
                    let _ = tx.try_send(PairEvent::Fingerprint(pairing::code_display(code)));
                });
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            let event = match result {
                Ok(o) if !o.peer_device_id.is_empty() => PairEvent::Paired(Target {
                    name: o.peer_name,
                    host,
                    port: o.session_port,
                    desktop_port,
                    device_id: o.peer_device_id,
                    secret: o.secret,
                }),
                Ok(_) => PairEvent::Failed(
                    "Host did not provide an authenticated identity; update it".into(),
                ),
                Err(e) => PairEvent::Failed(e.to_string()),
            };
            let _ = tx.try_send(event);
        });
        Ok(Self {
            socket: control,
            events: Mutex::new(rx),
            cancelled,
        })
    }
    pub fn poll(&self, timeout: Duration) -> Option<PairEvent> {
        self.events
            .lock()
            .unwrap()
            .recv_timeout(timeout.min(Duration::from_millis(500)))
            .ok()
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}
impl Drop for Pairing {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn pairing_wait() -> Duration {
    protocol::PAIR_DECISION_TIMEOUT + Duration::from_secs(10)
}

#[derive(Debug)]
pub enum Event {
    Capabilities {
        screen: bool,
        input: bool,
        text: bool,
    },
    Display {
        width: u32,
        height: u32,
    },
    Video {
        key: bool,
        bytes: Vec<u8>,
    },
    Status(String),
    Disconnected(String),
}
impl Event {
    /// Compact JNI boundary; bounds and authentication have already been checked.
    pub fn packet(self) -> Vec<u8> {
        match self {
            Self::Capabilities {
                screen,
                input,
                text,
            } => vec![4, u8::from(screen), u8::from(input), u8::from(text)],
            Self::Display { width, height } => {
                let mut p = vec![1];
                p.extend(width.to_be_bytes());
                p.extend(height.to_be_bytes());
                p
            }
            Self::Video { key, bytes } => {
                let mut p = Vec::with_capacity(bytes.len() + 2);
                p.extend([2, u8::from(key)]);
                p.extend(bytes);
                p
            }
            Self::Status(text) => {
                let mut p = vec![3];
                p.extend(text.bytes().take(2048));
                p
            }
            Self::Disconnected(text) => {
                let mut p = vec![0];
                p.extend(text.bytes().take(2048));
                p
            }
        }
    }
}

pub struct Session {
    commands: SyncSender<Message>,
    events: Mutex<Receiver<Event>>,
    socket: TcpStream,
    alive: Arc<AtomicBool>,
    input: bool,
    screen: bool,
    text: bool,
}
impl Session {
    pub fn connect(target: &Target, identity: &str) -> io::Result<Self> {
        target.validate()?;
        let mut caps = Capabilities::controller(Platform::Android);
        caps.clipboard = false; // Mobile never silently reads or writes its clipboard.
        let (channel, peer) = Channel::connect_with_capabilities(
            connect(&target.host, target.desktop_port)?,
            &secure::psk_from_secret(&target.secret),
            identity,
            &caps,
        )?;
        if peer != target.device_id {
            desktop::shutdown(&channel.shutdown);
            return Err(invalid("Authenticated device identity changed; pair again"));
        }
        let screen = channel.negotiated.screen;
        let input = channel.negotiated.input;
        let text = channel.negotiated.text_input;
        if !screen && !input {
            return Err(invalid("Host does not offer screen or input control"));
        }
        let socket = channel.shutdown.try_clone()?;
        let alive = Arc::new(AtomicBool::new(true));
        let (commands, rx) = mpsc::sync_channel(64);
        // Never discard an encoded reference frame: overflow disconnects the
        // session, so a fresh decoder/keyframe is required on the next connect.
        let (events, incoming) = mpsc::sync_channel(4);
        events
            .try_send(Event::Capabilities {
                screen,
                input,
                text,
            })
            .unwrap();
        let Channel {
            mut reader,
            mut writer,
            shutdown,
            ..
        } = channel;
        let read_alive = alive.clone();
        let read_socket = shutdown.try_clone()?;
        let read_events = events.clone();
        thread::spawn(move || {
            let mut assembly = desktop::Assembly::default();
            let mut waiting_key = true;
            let result = (|| -> io::Result<()> {
                while read_alive.load(Ordering::SeqCst) {
                    let event = match reader.recv()? {
                        Message::Display(d) if screen => {
                            if d.width > 1920 || d.height > 1080 {
                                return Err(invalid("Host exceeded negotiated mobile dimensions"));
                            }
                            assembly.clear();
                            waiting_key = true;
                            Some(Event::Display {
                                width: d.width,
                                height: d.height,
                            })
                        }
                        Message::Video {
                            id,
                            total,
                            offset,
                            key,
                            data,
                        } if screen => {
                            match assembly.push(id, total, offset, &data, 4 * 1024 * 1024)? {
                                Some(bytes) if !waiting_key || key => {
                                    waiting_key = false;
                                    Some(Event::Video { key, bytes })
                                }
                                _ => None,
                            }
                        }
                        Message::Status(s) => Some(Event::Status(s)),
                        Message::Heartbeat | Message::Pong(_) | Message::Stats(_) => None,
                        _ => return Err(invalid("Unexpected host message")),
                    };
                    if let Some(event) = event {
                        read_events
                            .try_send(event)
                            .map_err(|_| invalid("Decoder queue overflow; reconnect"))?;
                    }
                }
                Ok(())
            })();
            read_alive.store(false, Ordering::SeqCst);
            desktop::shutdown(&read_socket);
            let _ = read_events.try_send(Event::Disconnected(
                result
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Connection closed".into()),
            ));
        });
        let write_alive = alive.clone();
        thread::spawn(move || {
            let result = (|| -> io::Result<()> {
                if screen {
                    writer.send(&Message::Configure(protocol::stream::StreamSettings {
                        fps: 30,
                        max_width: 1920,
                        max_height: 1080,
                        bitrate_kbps: 8000,
                    }))?;
                }
                writer.send(&Message::Active {
                    visible: true,
                    clipboard: false,
                })?;
                while write_alive.load(Ordering::SeqCst) {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(m) => writer.send(&m)?,
                        Err(RecvTimeoutError::Timeout) => writer.send(&Message::Heartbeat)?,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                let _ = writer.send(&Message::ReleaseAll);
                let _ = writer.send(&Message::Active {
                    visible: false,
                    clipboard: false,
                });
                Ok(())
            })();
            write_alive.store(false, Ordering::SeqCst);
            desktop::shutdown(&shutdown);
            if let Err(e) = result {
                let _ = events.try_send(Event::Disconnected(e.to_string()));
            }
        });
        Ok(Self {
            commands,
            events: Mutex::new(incoming),
            socket,
            alive,
            input,
            screen,
            text,
        })
    }
    pub fn poll(&self, timeout: Duration) -> Option<Event> {
        match self
            .events
            .lock()
            .unwrap()
            .recv_timeout(timeout.min(Duration::from_millis(500)))
        {
            Ok(e) => Some(e),
            Err(_) if !self.alive.load(Ordering::SeqCst) => Some(Event::Disconnected(
                "Connection closed; reconnect explicitly".into(),
            )),
            Err(_) => None,
        }
    }
    fn send(&self, m: Message) -> io::Result<()> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(invalid("Not connected"));
        }
        self.commands.try_send(m).map_err(|_| {
            self.close();
            invalid("Input queue full or closed")
        })
    }
    pub fn input(&self, input: InputEvent) -> io::Result<()> {
        if !self.input {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Host input is disabled",
            ));
        }
        self.send(Message::Input(input))
    }
    pub fn absolute(&self, x: u16, y: u16) -> io::Result<()> {
        if !self.screen || !self.input {
            return Err(invalid("Absolute input requires an interactive screen"));
        }
        self.send(Message::Absolute { x, y })
    }
    pub fn text(&self, text: &str) -> io::Result<()> {
        if !self.text {
            return Err(invalid("Host does not support Unicode input; update it"));
        }
        if text.is_empty() || text.len() > 4096 || text.contains('\0') {
            return Err(invalid("Invalid text input"));
        }
        self.send(Message::TextInput(text.to_owned()))
    }
    pub fn release(&self) -> io::Result<()> {
        self.send(Message::ReleaseAll)
    }
    pub fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        desktop::shutdown(&self.socket);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}
