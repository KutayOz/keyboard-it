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
fn write_frame<W: Write>(w: &mut W, data: &[u8]) -> io::Result<()> {
    w.write_all(&(data.len() as u32).to_be_bytes())?;
    w.write_all(data)?;
    w.flush()
}

fn read_frame<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > MAX_FRAME || n > buf.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid frame length"));
    }
    r.read_exact(&mut buf[..n])?;
    Ok(n)
}

/// mac-sender side (TCP client = Noise initiator).
/// NNpsk0 = 2 messages:  -> e, psk   then   <- e, ee.
/// Call once right after TCP connect, before any KeyEvent.
pub fn handshake_initiator<S: Read + Write>(
    s: &mut S,
    psk: &[u8; 32],
) -> io::Result<snow::TransportState> {
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
        .psk(0, psk) // psk0: the PSK is mixed in BEFORE the first message
        .map_err(noise_err)?
        .build_initiator()
        .map_err(noise_err)?;
    let mut buf = [0u8; MAX_FRAME];
    // -> e, psk  (message 1) — payload: 1-byte protocol version (encrypted/authenticated)
    let n = hs.write_message(&[PROTOCOL_VERSION], &mut buf).map_err(noise_err)?;
    write_frame(s, &buf[..n])?;
    // <- e, ee   (message 2) — into_transport_mode() PANICS unless this is read first.
    // If the peer closes the connection here (wrong PSK or version rejection), replace
    // the raw UnexpectedEof with a diagnostic message; fail-closed behavior is unchanged.
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
    hs.read_message(&buf[..n], &mut tmp).map_err(noise_err)?;
    hs.into_transport_mode().map_err(noise_err)
}

/// win-receiver side (TCP server = Noise responder).
/// Call once right after TCP accept, before the read loop.
pub fn handshake_responder<S: Read + Write>(
    s: &mut S,
    psk: &[u8; 32],
) -> io::Result<snow::TransportState> {
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
        .psk(0, psk)
        .map_err(noise_err)?
        .build_responder()
        .map_err(noise_err)?;
    let mut buf = [0u8; MAX_FRAME];
    // <- e, psk  (message 1) — a wrong PSK fails here with a Decrypt error (fail-closed)
    let n = read_frame(s, &mut buf)?;
    let mut tmp = [0u8; MAX_FRAME];
    let m = hs.read_message(&buf[..n], &mut tmp).map_err(noise_err)?;
    // First payload byte = the peer's protocol version. An empty payload means an old
    // sender without the version field. Extra bytes are ignored for forward
    // compatibility. On mismatch, return an error that names the cause instead of a
    // silent connect/drop loop.
    let peer_ver = if m >= 1 { tmp[0] } else { 0 };
    if peer_ver != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "protocol version mismatch: local {PROTOCOL_VERSION}, peer {peer_ver} — update both sides"
            ),
        ));
    }
    // -> e, ee   (message 2)
    let n = hs.write_message(&[], &mut buf).map_err(noise_err)?;
    write_frame(s, &buf[..n])?;
    hs.into_transport_mode().map_err(noise_err)
}

/// Encrypted send: encode the InputEvent (key or mouse), encrypt, frame.
pub fn send_event<S: Write>(
    t: &mut snow::TransportState,
    s: &mut S,
    ev: &InputEvent,
) -> io::Result<()> {
    let (plain, plen) = ev.encode(); // ([u8; INPUT_MAX_LEN], usize)
    let mut ct = [0u8; INPUT_MAX_LEN + 16]; // plaintext + 16-byte Poly1305 tag
    let n = t.write_message(&plain[..plen], &mut ct).map_err(noise_err)?;
    write_frame(s, &ct[..n])
}

/// Encrypted receive. Nonce = the Noise transport counter: snow rejects out-of-order
/// or replayed frames automatically (replay protection for free). A closed connection
/// makes read_exact return UnexpectedEof. Length validation is per-variant inside
/// InputEvent::decode.
pub fn recv_event<S: Read>(
    t: &mut snow::TransportState,
    s: &mut S,
) -> io::Result<InputEvent> {
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

    #[test]
    fn handshake_version_ok_and_event_roundtrip() {
        let psk = psk_from_secret("test-key");
        let (mut ci, mut cr) = pair();
        let init = thread::spawn(move || {
            let mut t = handshake_initiator(&mut ci, &psk).unwrap();
            send_event(&mut t, &mut ci, &InputEvent::MouseMove { dx: 3, dy: -4 }).unwrap();
        });
        let mut t = handshake_responder(&mut cr, &psk).unwrap();
        assert_eq!(recv_event(&mut t, &mut cr).unwrap(), InputEvent::MouseMove { dx: 3, dy: -4 });
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
