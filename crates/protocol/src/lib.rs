//! Shared wire format — the common language of mac-sender and win-receiver.
//!
//! This is the actual "bridge": how an event is encoded is defined once here and
//! both binaries `use` the same types, so a protocol mismatch between them is a
//! compile-time error.

/// Noise (NNpsk0) layer that encrypts keystrokes on the LAN.
pub mod secure;

/// Persistent config: shared secret + peer host.
pub mod config;

/// One-click pairing: hands the shared secret to a new peer over an
/// unauthenticated Noise_NN channel, confirmed by a 6-digit code.
pub mod pairing;

/// Message type. Encoded as a single u8 tag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum MsgType {
    Down = 0,
    Up = 1,
    Repeat = 2,
}

impl MsgType {
    fn from_u8(b: u8) -> Result<Self, DecodeError> {
        match b {
            0 => Ok(MsgType::Down),
            1 => Ok(MsgType::Up),
            2 => Ok(MsgType::Repeat),
            _ => Err(DecodeError::BadMsgType(b)),
        }
    }
}

/// Fixed 5-byte wire record. Layout (big-endian = network order):
///   [0]      message type (u8)
///   [1..=2]  HID usage  (u16, USB HID Usage ID, Usage Page 0x07 — OS-independent)
///   [3..=4]  modifiers  (u16, bit mask: L/R Ctrl/Shift/Alt/GUI)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyEvent {
    pub msg: MsgType,
    pub hid_usage: u16,
    pub modifiers: u16,
}

/// Fixed length of one event on the wire.
pub const WIRE_LEN: usize = 5;

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    ShortBuffer,
    BadMsgType(u8),
    BadTag(u8),
}

impl KeyEvent {
    /// Encode into a fixed 5-byte array. No allocation, no serializer, deterministic width.
    pub fn encode(&self) -> [u8; WIRE_LEN] {
        let u = self.hid_usage.to_be_bytes();
        let m = self.modifiers.to_be_bytes();
        [self.msg as u8, u[0], u[1], m[0], m[1]]
    }

    /// Decode from the first full 5 bytes of `buf`.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        if buf.len() < WIRE_LEN {
            return Err(DecodeError::ShortBuffer);
        }
        Ok(KeyEvent {
            msg: MsgType::from_u8(buf[0])?,
            hid_usage: u16::from_be_bytes([buf[1], buf[2]]),
            modifiers: u16::from_be_bytes([buf[3], buf[4]]),
        })
    }
}

/// Modifier bit mask (matches the USB HID boot-keyboard modifier byte, widened to u16).
pub mod modmask {
    pub const LEFT_CTRL: u16 = 1 << 0;
    pub const LEFT_SHIFT: u16 = 1 << 1;
    pub const LEFT_ALT: u16 = 1 << 2;
    pub const LEFT_GUI: u16 = 1 << 3;
    pub const RIGHT_CTRL: u16 = 1 << 4;
    pub const RIGHT_SHIFT: u16 = 1 << 5;
    pub const RIGHT_ALT: u16 = 1 << 6;
    pub const RIGHT_GUI: u16 = 1 << 7;
}

// ===== Unified input event: wraps KeyEvent + mouse variants =====

/// Tag byte selecting the InputEvent variant on the wire.
pub mod tag {
    pub const KEY: u8 = 0;
    pub const MOUSE_MOVE: u8 = 1;
    pub const MOUSE_BUTTON: u8 = 2;
    pub const SCROLL: u8 = 3;
}

/// Mouse button id (small integer; shared by the wire and both sides).
pub mod mousebtn {
    pub const LEFT: u8 = 0;
    pub const RIGHT: u8 = 1;
    pub const MIDDLE: u8 = 2;
}

/// Unified input event. Wraps the existing KeyEvent unchanged; adds mouse variants.
/// Motion is RELATIVE (deltas): absolute Mac coordinates are meaningless on Windows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputEvent {
    Key(KeyEvent),
    MouseMove { dx: i16, dy: i16 },
    MouseButton { button: u8, down: bool }, // button: mousebtn::*
    Scroll { dx: i8, dy: i8 },              // dy: vertical, dx: horizontal (tick count)
}

/// Largest possible encoding: 1 tag byte + 5-byte Key payload.
pub const INPUT_MAX_LEN: usize = 1 + WIRE_LEN; // = 6

impl InputEvent {
    /// Encode into a fixed buffer; returns (buf, used_len). No heap.
    pub fn encode(&self) -> ([u8; INPUT_MAX_LEN], usize) {
        let mut b = [0u8; INPUT_MAX_LEN];
        let len = match self {
            InputEvent::Key(k) => {
                b[0] = tag::KEY;
                b[1..1 + WIRE_LEN].copy_from_slice(&k.encode());
                1 + WIRE_LEN
            }
            InputEvent::MouseMove { dx, dy } => {
                b[0] = tag::MOUSE_MOVE;
                b[1..3].copy_from_slice(&dx.to_be_bytes());
                b[3..5].copy_from_slice(&dy.to_be_bytes());
                5
            }
            InputEvent::MouseButton { button, down } => {
                b[0] = tag::MOUSE_BUTTON;
                b[1] = *button;
                b[2] = *down as u8;
                3
            }
            InputEvent::Scroll { dx, dy } => {
                b[0] = tag::SCROLL;
                b[1] = *dx as u8; // i8 -> u8, bit-preserving
                b[2] = *dy as u8;
                3
            }
        };
        (b, len)
    }

    /// Decode from the leading tag byte; each variant validates its own length.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let (&t, rest) = buf.split_first().ok_or(DecodeError::ShortBuffer)?;
        match t {
            tag::KEY => Ok(InputEvent::Key(KeyEvent::decode(rest)?)),
            tag::MOUSE_MOVE => {
                if rest.len() < 4 {
                    return Err(DecodeError::ShortBuffer);
                }
                Ok(InputEvent::MouseMove {
                    dx: i16::from_be_bytes([rest[0], rest[1]]),
                    dy: i16::from_be_bytes([rest[2], rest[3]]),
                })
            }
            tag::MOUSE_BUTTON => {
                if rest.len() < 2 {
                    return Err(DecodeError::ShortBuffer);
                }
                Ok(InputEvent::MouseButton {
                    button: rest[0],
                    down: rest[1] != 0,
                })
            }
            tag::SCROLL => {
                if rest.len() < 2 {
                    return Err(DecodeError::ShortBuffer);
                }
                Ok(InputEvent::Scroll {
                    dx: rest[0] as i8,
                    dy: rest[1] as i8,
                })
            }
            other => Err(DecodeError::BadTag(other)),
        }
    }
}

/// Default TCP port: mac-sender connects to it, win-receiver listens on it.
pub const DEFAULT_PORT: u16 = 5599;

/// mDNS/DNS-SD service type. Defined once here so the win-receiver advertiser
/// and the mac-sender browser can never drift apart on the service name.
pub const MDNS_SERVICE: &str = "_keyboard-it._tcp.local.";

/// TXT record key carrying the receiver's PAIRING port (not the session port —
/// that one is the SRV port). The Mac needs it before it has any key at all, so
/// it has to travel in the advertisement rather than over the session socket.
pub const MDNS_TXT_PAIR_PORT: &str = "pair";

/// TXT record key carrying `pairing::PAIRING_VERSION`, so a Mac can tell "too
/// old/new to pair with" apart from "unreachable" before opening a socket.
pub const MDNS_TXT_PAIR_VERSION: &str = "pairv";

/// Default pairing port: one above the session port. Only a hint — the receiver
/// falls back to an ephemeral port if this one is taken and always publishes the
/// port it actually got in the TXT record, so nothing depends on this value.
pub fn default_pairing_port(session_port: u16) -> u16 {
    session_port.wrapping_add(1)
}

/// How likely an advertised address is to actually reach the peer — lower is
/// better. Not a wire format: purely how a browser chooses among the A/AAAA
/// records a receiver published.
///
/// It exists because "any advertised address will do" is false. A receiver
/// running `enable_addr_auto()` publishes loopback and link-local records
/// alongside the one usable address; measured from the Mac, this PC advertised
///
/// ```text
/// [fe80::f9b9:…, ::1, ::5661:…, ::453e:…, 192.168.68.55]
/// ```
///
/// The browser survived that only by filtering to IPv4 before `min()` — and the
/// no-IPv4 fallback behind that filter was a live trap, because `::1` sorts below
/// every other address in the list. A receiver advertising no IPv4 at all would
/// have had the Mac dial its own loopback and wait out the timeout, with nothing
/// on either screen to say so. Ranking makes a junk address a last resort instead
/// of a coin flip.
pub fn addr_rank(a: &std::net::IpAddr) -> u8 {
    use std::net::IpAddr;
    match a {
        // Our own machine. Never the peer, whatever the peer advertised.
        _ if a.is_loopback() => 3,
        // 169.254/16: an interface that never got a lease, typically a dormant
        // virtual NIC. Advertised readily, reachable almost never.
        IpAddr::V4(v4) if v4.is_link_local() => 2,
        IpAddr::V4(_) => 0,
        // fe80::/10 means nothing without a scope id, which mDNS does not carry
        // and a `host:port` string cannot express — so it black-holes rather than
        // failing fast, which is the worst of both.
        IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80 => 2,
        IpAddr::V6(_) => 1,
    }
}

/// Pick the address to dial out of everything a receiver advertised.
///
/// Ties break on the address itself so the choice is stable across re-resolves:
/// a picker that churns can swap the target between the click and the connect.
pub fn pick_service_addr<'a, I>(addrs: I) -> Option<std::net::IpAddr>
where
    I: IntoIterator<Item = &'a std::net::IpAddr>,
{
    addrs.into_iter().min_by_key(|a| (addr_rank(a), **a)).copied()
}

use std::io::{self, Read, Write};

impl KeyEvent {
    /// Write the event as "4-byte big-endian length prefix + payload" (framed).
    /// Framing is not required for a fixed 5-byte record, but it lets the wire
    /// format grow later without breaking if fields are added.
    pub fn write_framed<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let payload = self.encode();
        w.write_all(&(payload.len() as u32).to_be_bytes())?;
        w.write_all(&payload)?;
        w.flush()
    }

    /// Read one framed event (length prefix + payload). Returns `UnexpectedEof`
    /// when the connection closes — callers interpret that as "peer went away".
    pub fn read_framed<R: Read>(r: &mut R) -> io::Result<KeyEvent> {
        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len == 0 || len > 64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid frame length"));
        }
        let mut payload = [0u8; 64];
        r.read_exact(&mut payload[..len])?;
        KeyEvent::decode(&payload[..len])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("decode error: {:?}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let e = KeyEvent {
            msg: MsgType::Down,
            hid_usage: 0x04, // 'A'
            modifiers: modmask::LEFT_SHIFT,
        };
        assert_eq!(e.encode().len(), WIRE_LEN);
        assert_eq!(KeyEvent::decode(&e.encode()).unwrap(), e);
    }

    #[test]
    fn short_buffer_errors() {
        assert_eq!(KeyEvent::decode(&[0u8; 3]), Err(DecodeError::ShortBuffer));
    }

    #[test]
    fn input_event_roundtrip() {
        let cases = [
            InputEvent::Key(KeyEvent { msg: MsgType::Down, hid_usage: 0x04, modifiers: modmask::LEFT_SHIFT }),
            InputEvent::MouseMove { dx: i16::MIN, dy: i16::MAX },
            InputEvent::MouseMove { dx: -1, dy: 7 },
            InputEvent::MouseButton { button: mousebtn::RIGHT, down: true },
            InputEvent::MouseButton { button: mousebtn::MIDDLE, down: false },
            InputEvent::Scroll { dx: -128, dy: 127 },
        ];
        for ev in cases {
            let (buf, len) = ev.encode();
            assert!(len <= INPUT_MAX_LEN);
            assert_eq!(InputEvent::decode(&buf[..len]).unwrap(), ev);
        }
    }

    #[test]
    fn input_event_bad_tag() {
        assert_eq!(InputEvent::decode(&[9u8, 0, 0]), Err(DecodeError::BadTag(9)));
        assert_eq!(InputEvent::decode(&[]), Err(DecodeError::ShortBuffer));
    }

    /// The address set an unpatched receiver published, as measured FROM THE MAC
    /// (`dns-sd -G v4v6 kutinho.local` agrees). One usable address out of five.
    #[test]
    fn one_usable_address_among_four_dead_ones() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        let from_the_mac = [
            ip("fe80::f9b9:dd17:228f:cff2"),
            ip("::1"),
            ip("::5661:6f66:5101:e5a4"),
            ip("::453e:653:18da:51e1"),
            ip("192.168.68.55"),
        ];
        assert_eq!(pick_service_addr(from_the_mac.iter()), Some(ip("192.168.68.55")));

        // The trap the old "filter to IPv4, else min()" code was one missing A
        // record away from: with no IPv4 to filter down to, `::1` is the minimum
        // of what remains, so the fallback dialled our own loopback.
        let no_ipv4: Vec<_> = from_the_mac.iter().copied().filter(|a| !a.is_ipv4()).collect();
        assert_eq!(no_ipv4.iter().min().copied(), Some(ip("::1")));
        // Ranked, loopback loses to the two routable-looking ones, and the
        // tie between those breaks on the address.
        assert_eq!(pick_service_addr(no_ipv4.iter()), Some(ip("::453e:653:18da:51e1")));

        // An IPv4 loopback is only ever visible to a browser on the same machine,
        // never over the LAN — but rank it anyway rather than rely on that.
        let local_view = [ip("192.168.68.55"), ip("127.0.0.1")];
        assert_eq!(pick_service_addr(local_view.iter()), Some(ip("192.168.68.55")));
        assert_eq!(local_view.iter().min().copied(), Some(ip("127.0.0.1")));
    }

    #[test]
    fn junk_addresses_lose_but_are_still_usable_alone() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        // A dormant virtual NIC must not outrank a real lease.
        let apipa = [ip("169.254.29.166"), ip("192.168.68.55")];
        assert_eq!(pick_service_addr(apipa.iter()), Some(ip("192.168.68.55")));
        // Routable IPv6 beats link-local IPv6...
        let v6 = [ip("fe80::1"), ip("2001:db8::5")];
        assert_eq!(pick_service_addr(v6.iter()), Some(ip("2001:db8::5")));
        // ...but any IPv4 beats any IPv6, and a lone bad address is still returned
        // rather than dropped: failing to connect beats "no peer found".
        let mixed = [ip("2001:db8::5"), ip("10.0.0.9")];
        assert_eq!(pick_service_addr(mixed.iter()), Some(ip("10.0.0.9")));
        let only_loopback = [ip("127.0.0.1")];
        assert_eq!(pick_service_addr(only_loopback.iter()), Some(ip("127.0.0.1")));
        assert_eq!(pick_service_addr([].iter()), None);
    }

    /// Re-resolves arrive in whatever order the daemon happens to emit them; the
    /// pick must not depend on that or the target can change between the click
    /// and the connect.
    #[test]
    fn pick_is_order_independent() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        let a = [ip("192.168.68.55"), ip("127.0.0.1"), ip("192.168.68.9")];
        let b = [ip("127.0.0.1"), ip("192.168.68.9"), ip("192.168.68.55")];
        assert_eq!(pick_service_addr(a.iter()), pick_service_addr(b.iter()));
        assert_eq!(pick_service_addr(a.iter()), Some(ip("192.168.68.9")));
    }

    #[test]
    fn framed_roundtrip() {
        use std::io::Cursor;
        let e = KeyEvent { msg: MsgType::Up, hid_usage: 0x0B /* 'h' */, modifiers: 0 };
        let mut buf = Vec::new();
        e.write_framed(&mut buf).unwrap();
        assert_eq!(buf.len(), 4 + WIRE_LEN); // length prefix + payload
        let mut cur = Cursor::new(buf);
        assert_eq!(KeyEvent::read_framed(&mut cur).unwrap(), e);
    }
}
