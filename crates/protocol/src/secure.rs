//! Encrypted + mutually-authenticated transport over Noise_NNpsk0.
//!
//! The existing "4-byte big-endian length prefix + payload" framing is kept as is;
//! the frame payload is now Noise ciphertext instead of a plain `KeyEvent::encode()`.
//! NNpsk0: no static keys — authentication rests entirely on "both sides know the
//! same PSK". A LAN eavesdropper without the PSK can neither complete the handshake
//! nor read a single keystroke.

use std::io::{self, Read, Write};

use crate::{InputEvent, INPUT_MAX_LEN};

const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const MAX_FRAME: usize = 65535; // Noise message ceiling (snow::constants::MAXMSGLEN)

/// Wire protocol version. Carried in the payload of the first handshake message
/// (encrypted + authenticated thanks to psk0). Bump on incompatible wire-format
/// changes so a version mix fails with a clear error instead of a silent
/// connect/drop loop.
pub const PROTOCOL_VERSION: u8 = 1;

fn noise_err(e: snow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("noise: {e:?}"))
}

/// Generate a fresh pairing key: 32 OS-random bytes as unpadded base64url
/// (43 chars, alphabet A-Za-z0-9-_). That alphabet needs no escaping in TOML
/// basic strings, shells, or GUI text fields, and no '=' padding can be lost
/// to copy/paste trimming. The key feeds psk_from_secret(), which hashes any
/// string, so the exact encoding is a UX choice, not a crypto one.
pub fn generate_key() -> String {
    let mut bytes = [0u8; 32];
    // OS RNG failure is unrecoverable and effectively impossible on the
    // supported desktop platforms; a panic beats silently weak keys.
    getrandom::fill(&mut bytes).expect("OS RNG unavailable");
    base64url_nopad(&bytes)
}

/// Minimal base64url (RFC 4648 section 5) encoder without padding — one call
/// site does not justify pulling a base64 crate into the workspace.
fn base64url_nopad(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        // Pack up to 3 bytes into a 24-bit group, zero-filling the tail.
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        // 1 input byte -> 2 output chars, 2 -> 3, 3 -> 4 (no '=' padding).
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Compress the user's passphrase (any length) into a fixed 32-byte PSK with
/// BLAKE2s-256. The domain-separation prefix keeps the same passphrase from
/// deriving the same PSK in another context.
pub fn psk_from_secret(secret: &str) -> [u8; 32] {
    use blake2::{Blake2s256, Digest};
    let mut h = Blake2s256::new();
    h.update(b"keyboard-it psk v1\0");
    h.update(secret.as_bytes());
    let mut psk = [0u8; 32];
    psk.copy_from_slice(&h.finalize());
    psk
}

/// Derive the PSK from config; fall back to the KEYBOARD_IT_KEY env var if the
/// config value is empty (backward compat/dev). The config file is the source of
/// truth; the env var is only a fallback.
pub fn psk_from_config_or_env(cfg: &crate::config::Config) -> io::Result<[u8; 32]> {
    if cfg.android_selected() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "ADB target does not use a native Noise session"));
    }
    if !cfg.shared_secret.is_empty() {
        return Ok(psk_from_secret(&cfg.shared_secret));
    }
    psk_from_env()
}

/// Read KEYBOARD_IT_KEY and derive the PSK. Missing/empty yields a clear error
/// (same in both binaries).
pub fn psk_from_env() -> io::Result<[u8; 32]> {
    match std::env::var("KEYBOARD_IT_KEY") {
        Ok(s) if !s.is_empty() => Ok(psk_from_secret(&s)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "KEYBOARD_IT_KEY is not set (use the SAME value on both machines)",
        )),
    }
}

// --- The existing 4-byte BE length framing, reused unchanged for ciphertext ---
// pub(crate) so the pairing handshake (pairing.rs) frames its messages exactly
// the same way instead of growing a second, subtly different framing.
pub(crate) fn write_frame<W: Write>(w: &mut W, data: &[u8]) -> io::Result<()> {
    w.write_all(&(data.len() as u32).to_be_bytes())?;
    w.write_all(data)?;
    w.flush()
}

pub(crate) fn read_frame<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > MAX_FRAME || n > buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    r.read_exact(&mut buf[..n])?;
    Ok(n)
}

/// mac-sender side (TCP client = Noise initiator).
/// NNpsk0 = 2 messages:  -> e, psk   then   <- e, ee.
/// Call once right after TCP connect, before any KeyEvent.
/// Optional desktop features are negotiated inside the authenticated Noise
/// payload. Old peers ignore the offer and reply empty, preserving protocol v1.
pub const FEATURE_CAPABILITIES: u32 = 1;
const EXTENSION: &[u8] = b"KI\x01";

fn feature_payload(features: u32) -> Vec<u8> {
    let mut out = EXTENSION.to_vec();
    out.extend(features.to_be_bytes());
    out
}
fn offered_features(payload: &[u8]) -> u32 {
    if payload.len() == 7 && payload.starts_with(EXTENSION) {
        u32::from_be_bytes(payload[3..7].try_into().unwrap())
    } else {
        0
    }
}

pub fn handshake_initiator<S: Read + Write>(
    s: &mut S,
    psk: &[u8; 32],
) -> io::Result<snow::TransportState> {
    handshake_initiator_with_features(s, psk, 0).map(|v| v.0)
}

pub fn handshake_initiator_with_features<S: Read + Write>(
    s: &mut S,
    psk: &[u8; 32],
    features: u32,
) -> io::Result<(snow::TransportState, u32)> {
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
        .psk(0, psk)
        .map_err(noise_err)?
        .build_initiator()
        .map_err(noise_err)?;
    let mut buf = [0u8; MAX_FRAME];
    let mut hello = vec![PROTOCOL_VERSION];
    if features != 0 {
        hello.extend(feature_payload(features));
    }
    let n = hs.write_message(&hello, &mut buf).map_err(noise_err)?;
    write_frame(s, &buf[..n])?;
    let n = read_frame(s, &mut buf).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted => io::Error::new(
            e.kind(),
            "peer rejected the handshake — is the pairing key identical on both machines?",
        ),
        _ => e,
    })?;
    let mut tmp = [0u8; MAX_FRAME];
    let m = hs.read_message(&buf[..n], &mut tmp).map_err(noise_err)?;
    let negotiated = offered_features(&tmp[..m]) & features;
    Ok((hs.into_transport_mode().map_err(noise_err)?, negotiated))
}

pub fn handshake_responder<S: Read + Write>(
    s: &mut S,
    psk: &[u8; 32],
) -> io::Result<snow::TransportState> {
    handshake_responder_many(s, &[*psk], 0).map(|v| v.0)
}

/// Try a bounded keyring against the same first authenticated frame. No new
/// cleartext key selector or wire format is needed; older controllers work too.
/// The caller must recheck the selected credential before granting input.
pub fn handshake_responder_many<S: Read + Write>(
    s: &mut S,
    psks: &[[u8; 32]],
    features: u32,
) -> io::Result<(snow::TransportState, usize, u32)> {
    if psks.len() > crate::access::MAX_CONTROLLERS + 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Controller keyring exceeds limit",
        ));
    }
    let mut buf = [0u8; 256];
    let n = read_frame(s, &mut buf)?;
    let mut tmp = [0u8; 256];
    for (index, psk) in psks.iter().enumerate() {
        let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
            .psk(0, psk)
            .map_err(noise_err)?
            .build_responder()
            .map_err(noise_err)?;
        let Ok(m) = hs.read_message(&buf[..n], &mut tmp) else {
            continue;
        };
        let peer_ver = tmp.first().copied().filter(|_| m > 0).unwrap_or(0);
        if peer_ver != PROTOCOL_VERSION {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!(
                "protocol version mismatch: local {PROTOCOL_VERSION}, peer {peer_ver} — update both sides")));
        }
        let negotiated = offered_features(&tmp[1..m]) & features;
        let response = if negotiated != 0 {
            feature_payload(negotiated)
        } else {
            Vec::new()
        };
        let n = hs.write_message(&response, &mut buf).map_err(noise_err)?;
        write_frame(s, &buf[..n])?;
        return Ok((
            hs.into_transport_mode().map_err(noise_err)?,
            index,
            negotiated,
        ));
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "No authorized controller key matched",
    ))
}

/// Encrypted send: encode the InputEvent (key or mouse), encrypt, frame.
pub fn send_event<S: Write>(
    t: &mut snow::TransportState,
    s: &mut S,
    ev: &InputEvent,
) -> io::Result<()> {
    let (plain, plen) = ev.encode(); // ([u8; INPUT_MAX_LEN], usize)
    let mut ct = [0u8; INPUT_MAX_LEN + 16]; // plaintext + 16-byte Poly1305 tag
    let n = t
        .write_message(&plain[..plen], &mut ct)
        .map_err(noise_err)?;
    write_frame(s, &ct[..n])
}

/// Encrypted receive. Nonce = the Noise transport counter: snow rejects out-of-order
/// or replayed frames automatically (replay protection for free). A closed connection
/// makes read_exact return UnexpectedEof. Length validation is per-variant inside
/// InputEvent::decode.
pub fn recv_event<S: Read>(t: &mut snow::TransportState, s: &mut S) -> io::Result<InputEvent> {
    let mut frame = [0u8; INPUT_MAX_LEN + 16];
    let n = read_frame(s, &mut frame)?;
    let mut plain = [0u8; INPUT_MAX_LEN + 16];
    let m = t.read_message(&frame[..n], &mut plain).map_err(noise_err)?;
    InputEvent::decode(&plain[..m])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("decode error: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// Set up a connected (client, server) socket pair over loopback.
    fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let c = thread::spawn(move || TcpStream::connect(addr).unwrap());
        let (s, _) = l.accept().unwrap();
        (c.join().unwrap(), s)
    }

    fn transports() -> (snow::TransportState, snow::TransportState) {
        let psk = psk_from_secret("random-test-fixture-only");
        let (mut client, mut server) = pair();
        let initiator = thread::spawn(move || handshake_initiator(&mut client, &psk).unwrap());
        let receiver = handshake_responder(&mut server, &psk).unwrap();
        (initiator.join().unwrap(), receiver)
    }

    #[test]
    fn modified_replayed_and_cross_session_ciphertext_is_rejected() {
        let (mut sender, mut receiver) = transports();
        let event = InputEvent::MouseMove { dx: 7, dy: -3 };
        let mut captured = Vec::new();
        send_event(&mut sender, &mut captured, &event).unwrap();
        assert_eq!(recv_event(&mut receiver, &mut io::Cursor::new(&captured)).unwrap(), event);
        // A nonce already consumed in this session cannot be used again.
        assert!(recv_event(&mut receiver, &mut io::Cursor::new(&captured)).is_err());
        let (_, mut fresh) = transports();
        assert!(recv_event(&mut fresh, &mut io::Cursor::new(&captured)).is_err());
        let (mut sender, mut receiver) = transports();
        let mut modified = Vec::new();
        send_event(&mut sender, &mut modified, &event).unwrap();
        *modified.last_mut().unwrap() ^= 1;
        assert!(recv_event(&mut receiver, &mut io::Cursor::new(modified)).is_err());
    }

    #[test]
    fn wrong_pairing_key_cannot_authenticate() {
        let (mut client, mut server) = pair();
        let initiator = thread::spawn(move || {
            handshake_initiator(&mut client, &psk_from_secret("attacker-key")).is_err()
        });
        assert!(handshake_responder(&mut server, &psk_from_secret("approved-key")).is_err());
        drop(server);
        assert!(initiator.join().unwrap());
    }

    #[test]
    fn base64url_nopad_known_vectors() {
        // RFC 4648 test vectors, '=' padding stripped, to pin the encoder.
        assert_eq!(base64url_nopad(b""), "");
        assert_eq!(base64url_nopad(b"f"), "Zg");
        assert_eq!(base64url_nopad(b"fo"), "Zm8");
        assert_eq!(base64url_nopad(b"foo"), "Zm9v");
        assert_eq!(base64url_nopad(b"foobar"), "Zm9vYmFy");
        // Bytes that hit the url-safe alphabet ('-' and '_', not '+'/'/').
        assert_eq!(base64url_nopad(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn generate_key_unique_length_and_charset() {
        let a = generate_key();
        let b = generate_key();
        assert_ne!(a, b, "two keys must differ");
        // 32 bytes -> ceil(32*8/6) = 43 chars unpadded.
        assert_eq!(a.len(), 43);
        // Alphabet must stay TOML-basic-string safe: no quote, backslash,
        // or control chars — nothing that would ever need escaping.
        for k in [&a, &b] {
            assert!(
                k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "unexpected char in key: {k}"
            );
        }
    }

    #[test]
    fn handshake_version_ok_and_event_roundtrip() {
        let psk = psk_from_secret("test-key");
        let (mut ci, mut cr) = pair();
        let init = thread::spawn(move || {
            let mut t = handshake_initiator(&mut ci, &psk).unwrap();
            send_event(&mut t, &mut ci, &InputEvent::MouseMove { dx: 3, dy: -4 }).unwrap();
        });
        let mut t = handshake_responder(&mut cr, &psk).unwrap();
        assert_eq!(
            recv_event(&mut t, &mut cr).unwrap(),
            InputEvent::MouseMove { dx: 3, dy: -4 }
        );
        init.join().unwrap();
    }

    #[test]
    fn versionless_legacy_sender_rejected_with_clear_error() {
        let psk = psk_from_secret("test-key");
        let (mut ci, mut cr) = pair();
        // Impersonate a legacy sender without the version field: empty payload.
        let init = thread::spawn(move || {
            let mut hs = snow::Builder::new(NOISE_PARAMS.parse().unwrap())
                .psk(0, &psk)
                .unwrap()
                .build_initiator()
                .unwrap();
            let mut buf = [0u8; MAX_FRAME];
            let n = hs.write_message(&[], &mut buf).unwrap();
            write_frame(&mut ci, &buf[..n]).unwrap();
        });
        let err = handshake_responder(&mut cr, &psk).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("protocol version mismatch"));
        init.join().unwrap();
    }
}
