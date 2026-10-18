//! Versioned remote-desktop channel. Kept separate from the legacy input wire format.
//! Every allocation and reassembly is bounded before accepting network data.
use std::io;
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const VERSION: u8 = 2;
pub const DEFAULT_PORT: u16 = 5601;
pub const CHUNK: usize = 60 * 1024;
pub const MAX_VIDEO: usize = 16 * 1024 * 1024;
pub const MAX_CLIPBOARD: usize = 1024 * 1024;
pub const TXT_PORT: &str = "desktop_port";
pub const TXT_VERSION: &str = "desktop_version";
pub const TXT_ID: &str = "device_id";
pub const TIMEOUT: Duration = Duration::from_secs(5);

fn invalid(s: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s.into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub source_width: u32,
    pub source_height: u32,
}
impl Display {
    pub fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= 8192
            && self.height <= 8192
            && self.source_width > 0
            && self.source_height > 0
            && self.source_width <= 32768
            && self.source_height <= 32768
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Hello {
        version: u8,
        device_id: String,
    },
    Display(Display),
    Active {
        visible: bool,
        clipboard: bool,
    },
    Input(crate::InputEvent),
    Absolute {
        x: u16,
        y: u16,
    },
    ReleaseAll,
    Video {
        id: u64,
        total: u32,
        offset: u32,
        key: bool,
        data: Vec<u8>,
    },
    Clipboard {
        id: u64,
        total: u32,
        offset: u32,
        data: Vec<u8>,
    },
    Heartbeat,
    Status(String),
    Configure(crate::stream::StreamSettings),
    /// Rates over the last reporting window, and host CPU wall times. These are
    /// not GPU completion timings or end-to-end input latency measurements.
    Stats(StreamStats),
    Ping(u64),
    Pong(u64),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    pub fps_milli: u32,
    pub bitrate_kbps: u32,
    pub capture_us: u32,
    pub encode_us: u32,
    pub send_us: u32,
    pub skipped: u32,
}
impl Message {
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut b = Vec::new();
        match self {
            Self::Hello { version, device_id } => {
                b.extend([0, *version]);
                b.extend(device_id.as_bytes());
            }
            Self::Display(d) => {
                b.push(1);
                for n in [
                    d.width,
                    d.height,
                    d.left as u32,
                    d.top as u32,
                    d.source_width,
                    d.source_height,
                ] {
                    b.extend(n.to_be_bytes());
                }
            }
            Self::Active { visible, clipboard } => b.extend([2, *visible as u8, *clipboard as u8]),
            Self::Input(e) => {
                b.push(3);
                let (v, n) = e.encode();
                b.extend(&v[..n]);
            }
            Self::Absolute { x, y } => {
                b.push(4);
                b.extend(x.to_be_bytes());
                b.extend(y.to_be_bytes());
            }
            Self::ReleaseAll => b.push(5),
            Self::Video {
                id,
                total,
                offset,
                key,
                data,
            } => {
                b.push(6);
                b.extend(id.to_be_bytes());
                b.extend(total.to_be_bytes());
                b.extend(offset.to_be_bytes());
                b.push(*key as u8);
                b.extend(data);
            }
            Self::Clipboard {
                id,
                total,
                offset,
                data,
            } => {
                b.push(7);
                b.extend(id.to_be_bytes());
                b.extend(total.to_be_bytes());
                b.extend(offset.to_be_bytes());
                b.extend(data);
            }
            Self::Heartbeat => b.push(8),
            Self::Status(s) => {
                b.push(9);
                b.extend(s.as_bytes());
            }
            Self::Configure(s) => {
                b.push(10);
                for n in [s.fps, s.max_width, s.max_height, s.bitrate_kbps] {
                    b.extend(n.to_be_bytes());
                }
            }
            Self::Stats(s) => {
                b.push(11);
                for n in [
                    s.fps_milli,
                    s.bitrate_kbps,
                    s.capture_us,
                    s.encode_us,
                    s.send_us,
                    s.skipped,
                ] {
                    b.extend(n.to_be_bytes());
                }
            }
            Self::Ping(id) | Self::Pong(id) => {
                b.push(if matches!(self, Self::Ping(_)) {
                    12
                } else {
                    13
                });
                b.extend(id.to_be_bytes());
            }
        }
        // Reuse decode validation for locally produced records too.
        Self::decode(&b)?;
        Ok(b)
    }
    pub fn decode(b: &[u8]) -> io::Result<Self> {
        if b.is_empty() || b.len() > CHUNK + 32 {
            return Err(invalid("desktop message size"));
        }
        let p = &b[1..];
        let u32_at = |i| u32::from_be_bytes(p[i..i + 4].try_into().unwrap());
        let u64_at = |i| u64::from_be_bytes(p[i..i + 8].try_into().unwrap());
        Ok(match b[0] {
            0 if p.len() >= 2 && p.len() <= 129 => Self::Hello {
                version: p[0],
                device_id: std::str::from_utf8(&p[1..])
                    .map_err(|_| invalid("identity UTF-8"))?
                    .into(),
            },
            1 if p.len() == 24 => {
                let d = Display {
                    width: u32_at(0),
                    height: u32_at(4),
                    left: u32_at(8) as i32,
                    top: u32_at(12) as i32,
                    source_width: u32_at(16),
                    source_height: u32_at(20),
                };
                if !d.valid() {
                    return Err(invalid("display dimensions"));
                }
                Self::Display(d)
            }
            2 if p.len() == 2 && p[0] <= 1 && p[1] <= 1 => Self::Active {
                visible: p[0] != 0,
                clipboard: p[1] != 0,
            },
            3 => {
                let e = crate::InputEvent::decode(p).map_err(|_| invalid("input"))?;
                if e.encode().1 != p.len() {
                    return Err(invalid("input length"));
                }
                if matches!(e, crate::InputEvent::MouseButton { button, .. } if button>2) {
                    return Err(invalid("mouse button"));
                }
                Self::Input(e)
            }
            4 if p.len() == 4 => Self::Absolute {
                x: u16::from_be_bytes(p[..2].try_into().unwrap()),
                y: u16::from_be_bytes(p[2..].try_into().unwrap()),
            },
            5 if p.is_empty() => Self::ReleaseAll,
            6 if p.len() > 17 && p[16] <= 1 => {
                chunk_bounds(u32_at(8), u32_at(12), p.len() - 17, MAX_VIDEO)?;
                Self::Video {
                    id: u64_at(0),
                    total: u32_at(8),
                    offset: u32_at(12),
                    key: p[16] != 0,
                    data: p[17..].into(),
                }
            }
            7 if p.len() >= 16 => {
                chunk_bounds(u32_at(8), u32_at(12), p.len() - 16, MAX_CLIPBOARD)?;
                Self::Clipboard {
                    id: u64_at(0),
                    total: u32_at(8),
                    offset: u32_at(12),
                    data: p[16..].into(),
                }
            }
            8 if p.is_empty() => Self::Heartbeat,
            9 if p.len() <= 1024 => Self::Status(
                std::str::from_utf8(p)
                    .map_err(|_| invalid("status UTF-8"))?
                    .into(),
            ),
            10 if p.len() == 16 => {
                let s = crate::stream::StreamSettings {
                    fps: u32_at(0),
                    max_width: u32_at(4),
                    max_height: u32_at(8),
                    bitrate_kbps: u32_at(12),
                };
                if !s.valid() {
                    return Err(invalid("invalid stream settings"));
                }
                Self::Configure(s)
            }
            11 if p.len() == 24 => Self::Stats(StreamStats {
                fps_milli: u32_at(0),
                bitrate_kbps: u32_at(4),
                capture_us: u32_at(8),
                encode_us: u32_at(12),
                send_us: u32_at(16),
                skipped: u32_at(20),
            }),
            12 if p.len() == 8 => Self::Ping(u64_at(0)),
            13 if p.len() == 8 => Self::Pong(u64_at(0)),
            _ => return Err(invalid("invalid desktop message")),
        })
    }
}
fn chunk_bounds(total: u32, offset: u32, len: usize, max: usize) -> io::Result<()> {
    if total as usize > max
        || len > CHUNK
        || (offset as usize)
            .checked_add(len)
            .is_none_or(|n| n > total as usize)
        || (len == 0 && total != 0)
    {
        return Err(invalid("invalid chunk bounds"));
    }
    Ok(())
}

/// Only contiguous chunks of one bounded transfer are retained. A new transfer
/// may supersede an incomplete frame, but must start at offset zero.
#[derive(Default)]
pub struct Assembly {
    current: Option<(u64, u32, Vec<u8>, Instant)>,
}
impl Assembly {
    pub fn push(
        &mut self,
        id: u64,
        total: u32,
        offset: u32,
        data: &[u8],
        max: usize,
    ) -> io::Result<Option<Vec<u8>>> {
        chunk_bounds(total, offset, data.len(), max)?;
        if offset == 0 {
            self.current = Some((
                id,
                total,
                Vec::with_capacity(total as usize),
                Instant::now(),
            ));
        }
        let Some((cur, size, bytes, started)) = &mut self.current else {
            return Err(invalid("chunk without start"));
        };
        if *cur != id
            || *size != total
            || bytes.len() != offset as usize
            || started.elapsed() > TIMEOUT
        {
            self.current = None;
            return Err(invalid("out-of-order or expired transfer"));
        }
        bytes.extend(data);
        if bytes.len() == total as usize {
            return Ok(self.current.take().map(|v| v.2));
        }
        Ok(None)
    }
    pub fn clear(&mut self) {
        self.current = None;
    }
}

pub fn video_chunks(id: u64, key: bool, data: &[u8]) -> io::Result<Vec<Message>> {
    if data.is_empty() || data.len() > MAX_VIDEO {
        return Err(invalid("video size"));
    }
    Ok(data
        .chunks(CHUNK)
        .enumerate()
        .map(|(i, p)| Message::Video {
            id,
            total: data.len() as u32,
            offset: (i * CHUNK) as u32,
            key,
            data: p.into(),
        })
        .collect())
}
pub fn clipboard_chunks(id: u64, text: &str) -> io::Result<Vec<Message>> {
    if text.len() > MAX_CLIPBOARD {
        return Err(invalid("clipboard exceeds 1 MiB"));
    }
    if text.is_empty() {
        return Ok(vec![Message::Clipboard {
            id,
            total: 0,
            offset: 0,
            data: vec![],
        }]);
    }
    Ok(text
        .as_bytes()
        .chunks(CHUNK)
        .enumerate()
        .map(|(i, p)| Message::Clipboard {
            id,
            total: text.len() as u32,
            offset: (i * CHUNK) as u32,
            data: p.into(),
        })
        .collect())
}

/// Domain separation prevents a desktop endpoint being mistaken for a legacy
/// input endpoint, even when both use the same saved pairing secret.
fn desktop_psk(psk: &[u8; 32]) -> [u8; 32] {
    use blake2::{Blake2s256, Digest};
    let mut h = Blake2s256::new();
    h.update(b"keyboard-it desktop v1\0");
    h.update(psk);
    h.finalize().into()
}
pub struct Reader {
    stream: TcpStream,
    crypto: Arc<Mutex<snow::TransportState>>,
}
pub struct Writer {
    stream: TcpStream,
    crypto: Arc<Mutex<snow::TransportState>>,
}
pub struct Channel {
    pub reader: Reader,
    pub writer: Writer,
    pub shutdown: TcpStream,
}
impl Channel {
    pub fn connect(
        mut stream: TcpStream,
        psk: &[u8; 32],
        id: &str,
        initiator: bool,
    ) -> io::Result<(Self, String)> {
        // Accepted sockets may inherit the listener's nonblocking flag (BSD /
        // Winsock). Framed handshake reads require blocking I/O with deadlines.
        stream.set_nonblocking(false)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let psk = desktop_psk(psk);
        let transport = if initiator {
            crate::secure::handshake_initiator(&mut stream, &psk)?
        } else {
            crate::secure::handshake_responder(&mut stream, &psk)?
        };
        let crypto = Arc::new(Mutex::new(transport));
        let mut c = Self {
            reader: Reader {
                stream: stream.try_clone()?,
                crypto: crypto.clone(),
            },
            writer: Writer {
                stream: stream.try_clone()?,
                crypto,
            },
            shutdown: stream,
        };
        c.writer.send(&Message::Hello {
            version: VERSION,
            device_id: id.into(),
        })?;
        match c.reader.recv()? {
            Message::Hello { version, device_id }
                if version == VERSION && !device_id.is_empty() =>
            {
                Ok((c, device_id))
            }
            _ => Err(invalid("desktop version mismatch — update both apps")),
        }
    }
}
impl Reader {
    pub fn recv(&mut self) -> io::Result<Message> {
        let mut ct = vec![0; 65535];
        let n = crate::secure::read_frame(&mut self.stream, &mut ct)?;
        let mut plain = vec![0; n];
        let len = self
            .crypto
            .lock()
            .unwrap()
            .read_message(&ct[..n], &mut plain)
            .map_err(|e| invalid(format!("desktop authentication: {e}")))?;
        Message::decode(&plain[..len])
    }
}
impl Writer {
    pub fn send(&mut self, msg: &Message) -> io::Result<()> {
        let plain = msg.encode()?;
        let mut ct = vec![0; plain.len() + 16];
        let len = self
            .crypto
            .lock()
            .unwrap()
            .write_message(&plain, &mut ct)
            .map_err(|e| invalid(format!("desktop encryption: {e}")))?;
        crate::secure::write_frame(&mut self.stream, &ct[..len])
    }
}
pub fn shutdown(s: &TcpStream) {
    let _ = s.shutdown(Shutdown::Both);
}

/// Clipboard change tracking, with a baseline on activation/reconnect. Applying
/// remote content records it before touching the OS clipboard to suppress echoes.
#[derive(Default)]
pub struct ClipboardSync {
    baseline: Option<String>,
    last_remote_id: Option<u64>,
}
impl ClipboardSync {
    pub fn reset(&mut self, current: Option<String>) {
        self.baseline = current;
        self.last_remote_id = None;
    }
    pub fn local_change(&mut self, current: Option<String>) -> Option<String> {
        // A busy OS clipboard (or a file/image selection) is not an empty text
        // clipboard and must not erase the echo-suppression baseline.
        current.as_ref()?;
        if current == self.baseline {
            return None;
        }
        self.baseline = current.clone();
        current.filter(|s| s.len() <= MAX_CLIPBOARD)
    }
    pub fn remote(&mut self, id: u64, text: &str) -> bool {
        if self.last_remote_id == Some(id) {
            return false;
        }
        self.last_remote_id = Some(id);
        self.baseline = Some(text.into());
        true
    }
}

/// Fit an image into a view without stretching; normalize clicks to 0..65535.
/// AppKit's coordinates start at the bottom, Windows' at the top.
pub fn map_pointer(x: f64, y: f64, vw: f64, vh: f64, iw: u32, ih: u32) -> Option<(u16, u16)> {
    if vw <= 0.0 || vh <= 0.0 || iw == 0 || ih == 0 {
        return None;
    }
    let scale = (vw / iw as f64).min(vh / ih as f64);
    let w = iw as f64 * scale;
    let h = ih as f64 * scale;
    let x = x - (vw - w) / 2.;
    let y = y - (vh - h) / 2.;
    if !(0.0..=w).contains(&x) || !(0.0..=h).contains(&y) {
        return None;
    }
    Some((
        (x / w * 65535.).round() as u16,
        ((1. - y / h) * 65535.).round() as u16,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_roundtrip_and_large_frame() {
        let data = vec![7; 200_000];
        let mut a = Assembly::default();
        let mut done = None;
        for m in video_chunks(42, true, &data).unwrap() {
            if let Message::Video {
                id,
                total,
                offset,
                data,
                ..
            } = Message::decode(&m.encode().unwrap()).unwrap()
            {
                done = a.push(id, total, offset, &data, MAX_VIDEO).unwrap();
            }
        }
        assert_eq!(done.unwrap(), data);
    }
    #[test]
    fn rejects_bad_lengths_and_order() {
        for b in [vec![], vec![1, 0], vec![6; 18], vec![4, 1], vec![2, 2, 0]] {
            assert!(Message::decode(&b).is_err());
        }
        let mut a = Assembly::default();
        assert!(a.push(1, 4, 1, b"a", MAX_VIDEO).is_err());
        a.push(1, 4, 0, b"ab", MAX_VIDEO).unwrap();
        assert!(a.push(1, 4, 1, b"cd", MAX_VIDEO).is_err());
        assert!(a
            .push(1, (MAX_CLIPBOARD + 1) as u32, 0, b"a", MAX_CLIPBOARD)
            .is_err());
    }
    #[test]
    fn clipboard_utf8_empty_and_no_echo_or_reconnect_replay() {
        let text = "İstanbul 🖥️".repeat(9000);
        let mut a = Assembly::default();
        let mut output = None;
        for m in clipboard_chunks(9, &text).unwrap() {
            if let Message::Clipboard {
                id,
                total,
                offset,
                data,
            } = m
            {
                output = a.push(id, total, offset, &data, MAX_CLIPBOARD).unwrap();
            }
        }
        assert_eq!(String::from_utf8(output.unwrap()).unwrap(), text);
        assert_eq!(clipboard_chunks(1, "").unwrap().len(), 1);
        let mut s = ClipboardSync::default();
        s.reset(Some("old".into()));
        assert_eq!(s.local_change(Some("old".into())), None);
        assert!(s.remote(1, "new"));
        assert!(!s.remote(1, "new"));
        assert_eq!(s.local_change(Some("new".into())), None);
        assert_eq!(s.local_change(Some("İ".into())), Some("İ".into()));
    }
    #[test]
    fn pointer_letterbox_retina_and_edges() {
        assert_eq!(
            map_pointer(500., 500., 1000., 1000., 1920, 1080),
            Some((32768, 32768))
        );
        assert_eq!(map_pointer(500., 20., 1000., 1000., 1920, 1080), None);
        assert_eq!(
            map_pointer(0., 1000., 1000., 1000., 1000, 1000),
            Some((0, 0))
        );
    }
    #[test]
    fn encrypted_duplex_wrong_key_and_version() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let p = [9; 32];
        let t = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let (mut c, id) = Channel::connect(s, &p, "pc", false).unwrap();
            assert_eq!(id, "mac");
            assert_eq!(c.reader.recv().unwrap(), Message::Heartbeat);
            c.writer.send(&Message::ReleaseAll).unwrap();
        });
        let (mut c, id) =
            Channel::connect(TcpStream::connect(addr).unwrap(), &p, "mac", true).unwrap();
        assert_eq!(id, "pc");
        c.writer.send(&Message::Heartbeat).unwrap();
        assert_eq!(c.reader.recv().unwrap(), Message::ReleaseAll);
        t.join().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            Channel::connect(l.accept().unwrap().0, &[8; 32], "pc", false).is_err()
        });
        assert!(Channel::connect(TcpStream::connect(addr).unwrap(), &p, "mac", true).is_err());
        assert!(t.join().unwrap());
        assert_ne!(desktop_psk(&p), p);
    }
}

#[cfg(test)]
mod resilience_tests {
    use super::*;
    #[test]
    fn incompatible_desktop_version_is_rejected_after_authentication() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let psk = [3; 32];
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut crypto =
                crate::secure::handshake_responder(&mut socket, &desktop_psk(&psk)).unwrap();
            let plain = Message::Hello {
                version: VERSION + 1,
                device_id: "pc".into(),
            }
            .encode()
            .unwrap();
            let mut ciphertext = vec![0; plain.len() + 16];
            let len = crypto.write_message(&plain, &mut ciphertext).unwrap();
            crate::secure::write_frame(&mut socket, &ciphertext[..len]).unwrap();
            let mut bytes = [0; 65535];
            let _ = crate::secure::read_frame(&mut socket, &mut bytes);
        });
        let result = Channel::connect(TcpStream::connect(address).unwrap(), &psk, "mac", true);
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("version mismatch"));
        thread.join().unwrap();
    }
    #[test]
    fn expired_and_superseded_transfers_do_not_reuse_partial_bytes() {
        let mut a = Assembly::default();
        a.push(1, 4, 0, b"ab", MAX_VIDEO).unwrap();
        a.current.as_mut().unwrap().3 = Instant::now() - TIMEOUT - Duration::from_secs(1);
        assert!(a.push(1, 4, 2, b"cd", MAX_VIDEO).is_err());
        a.push(2, 4, 0, b"ab", MAX_VIDEO).unwrap();
        assert_eq!(
            a.push(3, 2, 0, b"xy", MAX_VIDEO).unwrap(),
            Some(b"xy".to_vec())
        );
    }
    #[test]
    fn metadata_and_all_control_records_roundtrip() {
        assert!(h264_keyframe(&[0, 0, 0, 1, 0x65, 1]));
        assert!(h264_keyframe(&[0, 0, 1, 0x65, 1]));
        assert!(!h264_keyframe(&[0, 0, 1, 0x41, 1]));
        let messages = [
            Message::Configure(crate::stream::StreamSettings::PROFILES[2].1),
            Message::Stats(StreamStats {
                fps_milli: 59800,
                bitrate_kbps: 19400,
                capture_us: 800,
                encode_us: 1500,
                send_us: 400,
                skipped: 1,
            }),
            Message::Ping(100),
            Message::Pong(100),
            Message::Display(Display {
                width: 1920,
                height: 1080,
                left: -1920,
                top: 0,
                source_width: 1920,
                source_height: 1080,
            }),
            Message::Active {
                visible: true,
                clipboard: false,
            },
            Message::Absolute { x: 65535, y: 0 },
            Message::ReleaseAll,
            Message::Heartbeat,
            Message::Status("Ekran kilitli".into()),
            Message::Input(crate::InputEvent::MouseButton {
                button: 2,
                down: false,
            }),
        ];
        for m in messages {
            assert_eq!(Message::decode(&m.encode().unwrap()).unwrap(), m);
        }
        assert!(Message::Configure(crate::stream::StreamSettings {
            fps: u32::MAX,
            ..Default::default()
        })
        .encode()
        .is_err());
        for tag in [10, 11, 12, 13] {
            assert!(Message::decode(&[tag, 0]).is_err());
        }
    }
}

/// Identify IDR access units without assuming three- or four-byte start codes.
pub fn h264_keyframe(data: &[u8]) -> bool {
    data.windows(4)
        .any(|w| w[..3] == [0, 0, 1] && w[3] & 31 == 5)
}
