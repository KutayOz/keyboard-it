//! One-click pairing: how two machines that share NO secret get one.
//!
//! The session transport (`secure.rs`) is `Noise_NNpsk0` — it authenticates purely
//! on "both sides already know the same PSK", and the PSK is mixed in *before*
//! message 1. An unpaired Mac therefore cannot decrypt a single byte of it, so
//! pairing cannot ride on that channel; it needs one of its own.
//!
//! This module uses plain `Noise_NN`: ephemeral keys on both sides, no static
//! keys, no PSK. That yields an ENCRYPTED but UNAUTHENTICATED channel — safe
//! against a passive eavesdropper, but not against someone actively sitting in
//! the middle relaying both halves.
//!
//! The user closes that hole. Both sides derive a 6-digit code from the Noise
//! handshake hash, which commits to both ephemeral public keys. A man in the
//! middle has to run two separate handshakes, so it cannot make both codes come
//! out the same: the receiver's "Allow" dialog shows a code that would not match
//! the one on the sender's screen. One click both authorizes the pairing and
//! confirms the channel.
//!
//! Wire (all frames use secure.rs's 4-byte big-endian length prefix):
//! ```text
//!   initiator                                    responder
//!     -> e                  [PAIRING_VERSION]      (cleartext: NN msg 1 has no key yet)
//!     <- e, ee              [PAIRING_VERSION]      (encrypted)
//!     -- both: h = handshake hash -> code; then transport mode --
//!     -> HELLO (enc)        [u8 len][name UTF-8]
//!                                                  responder asks the user
//!     <- RESULT (enc)       [0x00]                                = declined
//!                           [0x01][u8 len][secret][u16 BE port][u8 len][name]
//! ```
//!
//! Timeouts are the caller's job: set them on the socket before calling in.
//! Both functions are blocking, and the responder blocks for as long as the
//! `decide` callback takes to return (a human clicking a button).

use std::io::{self, Read, Write};

use crate::secure::{read_frame, write_frame};

/// Pairing wire version. Independent of `secure::PROTOCOL_VERSION`: the session
/// format and the pairing format can move separately.
pub const PAIRING_VERSION: u8 = 1;

/// No PSK and no static keys — the whole point is that nothing is shared yet.
const NOISE_PARAMS: &str = "Noise_NN_25519_ChaChaPoly_BLAKE2s";

/// Generous ceiling for a handshake message (an NN message is ~48 bytes).
/// `read_frame` rejects anything longer, so this doubles as a sanity limit.
const MAX_HANDSHAKE: usize = 256;

/// Ceiling for a post-handshake pairing message. The largest is RESULT:
/// 1 + 1 + 255 + 2 + 1 + 255 = 515 bytes.
const MAX_MSG: usize = 1024;

/// Longest accepted device name, in bytes. Names are display-only, and this
/// keeps a single length byte sufficient.
pub const MAX_NAME_LEN: usize = 64;

const RESULT_DECLINED: u8 = 0x00;
const RESULT_ACCEPTED: u8 = 0x01;

fn noise_err(e: snow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("noise: {e:?}"))
}

fn bad_data(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn version_mismatch(peer: u8) -> io::Error {
    bad_data(&format!(
        "pairing version mismatch: local {PAIRING_VERSION}, peer {peer} — update both machines"
    ))
}

/// What the receiver learned about the machine asking to pair. Shown in the
/// confirmation dialog; `code` is what the user compares against the sender's
/// screen.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PairRequest {
    /// The initiator's self-reported device name. UNTRUSTED text from the
    /// network — already sanitized to printable characters by `pair_responder`,
    /// but still chosen by the peer, so never treat it as an identity.
    pub peer_name: String,
    /// 6 decimal digits, e.g. "482913".
    pub code: String,
}

/// The receiver's answer, carrying the request either way so the caller can log
/// or rate-limit on it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PairDecision {
    Accepted(PairRequest),
    Declined(PairRequest),
}

/// What the sender walks away with: everything needed to open a session.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PairOutcome {
    /// The receiver's pairing key, to be stored in `Config::shared_secret`.
    pub secret: String,
    /// The port the receiver's SESSION listener is on (not the pairing port).
    pub session_port: u16,
    /// The receiver's display name, for the UI.
    pub peer_name: String,
    /// The 6-digit code that was shown to the user.
    pub code: String,
}

/// Render a code for humans: "482913" -> "482 913". Easier to read aloud and to
/// compare across two screens.
pub fn code_display(code: &str) -> String {
    match code.split_at_checked(3) {
        Some((a, b)) => format!("{a} {b}"),
        None => code.to_string(),
    }
}

/// Derive the short authentication string from the Noise handshake hash.
///
/// The hash commits to both ephemeral public keys and every payload exchanged,
/// so two independent handshakes (which is what a man in the middle is forced
/// to run) cannot agree on it.
fn sas_from_hash(h: &[u8]) -> String {
    use blake2::{Blake2s256, Digest};
    let mut d = Blake2s256::new();
    d.update(b"keyboard-it sas v1\0");
    d.update(h);
    let out = d.finalize();
    let n = u32::from_be_bytes([out[0], out[1], out[2], out[3]]) % 1_000_000;
    format!("{n:06}")
}

/// Clamp a network-supplied device name to something safe to put in a dialog:
/// drop control characters (no newlines faking extra dialog text, no ANSI
/// escapes in a terminal), then cut to MAX_NAME_LEN on a char boundary.
fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    let mut out = String::with_capacity(cleaned.len().min(MAX_NAME_LEN));
    for c in cleaned.chars() {
        if out.len() + c.len_utf8() > MAX_NAME_LEN {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "unknown device".to_string()
    } else {
        out
    }
}

/// Encode a length-prefixed string. Callers keep names within MAX_NAME_LEN and
/// the secret is 43 bytes, so the u8 length can never overflow in practice;
/// truncation here is a belt-and-braces guard rather than expected behavior.
fn push_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(u8::MAX as usize);
    buf.push(n as u8);
    buf.extend_from_slice(&bytes[..n]);
}

/// Read a length-prefixed string, advancing `rest`.
fn take_str(rest: &mut &[u8], what: &str) -> io::Result<String> {
    let (&len, tail) = rest.split_first().ok_or_else(|| bad_data(&format!("truncated {what}")))?;
    let len = len as usize;
    if tail.len() < len {
        return Err(bad_data(&format!("truncated {what}")));
    }
    let (s, tail) = tail.split_at(len);
    *rest = tail;
    String::from_utf8(s.to_vec()).map_err(|_| bad_data(&format!("{what} is not valid UTF-8")))
}

fn send_msg<S: Write>(t: &mut snow::TransportState, s: &mut S, plain: &[u8]) -> io::Result<()> {
    let mut ct = [0u8; MAX_MSG + 16];
    let n = t.write_message(plain, &mut ct).map_err(noise_err)?;
    write_frame(s, &ct[..n])
}

fn recv_msg<S: Read>(
    t: &mut snow::TransportState,
    s: &mut S,
    out: &mut [u8; MAX_MSG],
) -> io::Result<usize> {
    let mut ct = [0u8; MAX_MSG + 16];
    let n = read_frame(s, &mut ct)?;
    t.read_message(&ct[..n], out).map_err(noise_err)
}

/// Sender side (TCP client). Runs the NN handshake, reports the 6-digit code
/// through `on_code` so the UI can show it while the user walks to the other
/// machine, then BLOCKS until the receiver answers.
///
/// A decline comes back as `ErrorKind::PermissionDenied`. Give the socket a read
/// timeout long enough to cover a human decision (the receiver auto-declines at
/// 60 s, so ~90 s is the natural choice).
pub fn pair_initiator<S: Read + Write>(
    s: &mut S,
    my_name: &str,
    on_code: impl FnOnce(&str),
) -> io::Result<PairOutcome> {
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
        .build_initiator()
        .map_err(noise_err)?;
    let mut buf = [0u8; MAX_HANDSHAKE];

    // -> e  (message 1). NN has no key material yet, so this payload travels in
    // the clear; it carries nothing but the version.
    let n = hs.write_message(&[PAIRING_VERSION], &mut buf).map_err(noise_err)?;
    write_frame(s, &buf[..n])?;

    // <- e, ee  (message 2), now encrypted.
    // The receiver hangs up here when it is already showing a dialog for someone
    // else, or is cooling down after a decline — both temporary, both fixed by
    // trying again, so say that rather than implying the PC is unreachable.
    let n = read_frame(s, &mut buf).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted => io::Error::new(
            e.kind(),
            "the PC turned down the pairing request — it may be busy with another one. Try again.",
        ),
        _ => e,
    })?;
    let mut tmp = [0u8; MAX_HANDSHAKE];
    let m = hs.read_message(&buf[..n], &mut tmp).map_err(noise_err)?;
    let peer_ver = if m >= 1 { tmp[0] } else { 0 };
    if peer_ver != PAIRING_VERSION {
        return Err(version_mismatch(peer_ver));
    }

    // The hash lives on HandshakeState only — capture it before transport mode
    // consumes it.
    let code = sas_from_hash(hs.get_handshake_hash());
    let mut t = hs.into_transport_mode().map_err(noise_err)?;

    // The user needs the code NOW: they are about to compare it on the other
    // screen, and the next read blocks until they click.
    on_code(&code);

    let mut hello = Vec::with_capacity(1 + my_name.len());
    push_str(&mut hello, &sanitize_name(my_name));
    send_msg(&mut t, s, &hello)?;

    let mut out = [0u8; MAX_MSG];
    let n = recv_msg(&mut t, s, &mut out).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted => io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pairing was not confirmed on the PC",
        ),
        _ => e,
    })?;
    let mut rest = &out[..n];
    let (&status, tail) = rest.split_first().ok_or_else(|| bad_data("empty pairing result"))?;
    rest = tail;
    if status == RESULT_DECLINED {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pairing was declined on the PC",
        ));
    }
    if status != RESULT_ACCEPTED {
        return Err(bad_data("unrecognized pairing result"));
    }

    let secret = take_str(&mut rest, "pairing key")?;
    if secret.is_empty() {
        return Err(bad_data("the PC sent an empty pairing key"));
    }
    if rest.len() < 2 {
        return Err(bad_data("truncated session port"));
    }
    let session_port = u16::from_be_bytes([rest[0], rest[1]]);
    rest = &rest[2..];
    if session_port == 0 {
        return Err(bad_data("the PC sent an invalid session port"));
    }
    let peer_name = sanitize_name(&take_str(&mut rest, "PC name")?);

    Ok(PairOutcome { secret, session_port, peer_name, code })
}

/// Receiver side (TCP server). Runs the NN handshake, reads the sender's name,
/// then calls `decide` with the request — that callback is expected to put the
/// name and code in front of the user and block until they answer (or time out
/// into `false`).
///
/// On accept, `secret` is handed over verbatim; the caller owns generating and
/// persisting it. Returns what happened either way so the caller can log it and
/// rate-limit repeat prompts.
pub fn pair_responder<S: Read + Write>(
    s: &mut S,
    secret: &str,
    session_port: u16,
    my_name: &str,
    decide: impl FnOnce(&PairRequest) -> bool,
) -> io::Result<PairDecision> {
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
        .build_responder()
        .map_err(noise_err)?;
    let mut buf = [0u8; MAX_HANDSHAKE];

    // <- e  (message 1), cleartext payload.
    let n = read_frame(s, &mut buf)?;
    let mut tmp = [0u8; MAX_HANDSHAKE];
    let m = hs.read_message(&buf[..n], &mut tmp).map_err(noise_err)?;
    let peer_ver = if m >= 1 { tmp[0] } else { 0 };

    // Answer even on a mismatch, so the peer can report the real reason instead
    // of a bare connection drop; then fail.
    let n = hs.write_message(&[PAIRING_VERSION], &mut buf).map_err(noise_err)?;
    write_frame(s, &buf[..n])?;
    if peer_ver != PAIRING_VERSION {
        return Err(version_mismatch(peer_ver));
    }

    let code = sas_from_hash(hs.get_handshake_hash());
    let mut t = hs.into_transport_mode().map_err(noise_err)?;

    let mut out = [0u8; MAX_MSG];
    let n = recv_msg(&mut t, s, &mut out)?;
    let mut rest = &out[..n];
    let peer_name = sanitize_name(&take_str(&mut rest, "device name")?);
    let req = PairRequest { peer_name, code };

    if !decide(&req) {
        // Best-effort: the peer may already be gone, and a decline is not a
        // failure of ours to report.
        let _ = send_msg(&mut t, s, &[RESULT_DECLINED]);
        return Ok(PairDecision::Declined(req));
    }

    let mut msg = Vec::with_capacity(64 + my_name.len());
    msg.push(RESULT_ACCEPTED);
    push_str(&mut msg, secret);
    msg.extend_from_slice(&session_port.to_be_bytes());
    push_str(&mut msg, &sanitize_name(my_name));
    send_msg(&mut t, s, &msg)?;

    Ok(PairDecision::Accepted(req))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// Connected (client, server) socket pair over loopback — same helper shape
    /// as secure.rs's tests.
    fn socket_pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let c = thread::spawn(move || TcpStream::connect(addr).unwrap());
        let (s, _) = l.accept().unwrap();
        (c.join().unwrap(), s)
    }

    #[test]
    fn accept_transfers_the_secret_and_both_sides_show_the_same_code() {
        let (mut ci, mut cr) = socket_pair();
        let initiator = thread::spawn(move || {
            let mut seen = String::new();
            let outcome =
                pair_initiator(&mut ci, "Kutay's MacBook", |c| seen = c.to_string()).unwrap();
            (outcome, seen)
        });

        let decision =
            pair_responder(&mut cr, "s3cret-key", 5599, "DESKTOP-ABC", |_| true).unwrap();
        let (outcome, seen_by_initiator) = initiator.join().unwrap();

        assert_eq!(outcome.secret, "s3cret-key");
        assert_eq!(outcome.session_port, 5599);
        assert_eq!(outcome.peer_name, "DESKTOP-ABC");

        let PairDecision::Accepted(req) = decision else { panic!("expected Accepted") };
        assert_eq!(req.peer_name, "Kutay's MacBook");
        // The whole security argument: one code, derived independently on both
        // ends, and the sender's UI saw exactly what it later reports.
        assert_eq!(req.code, outcome.code);
        assert_eq!(seen_by_initiator, outcome.code);
    }

    #[test]
    fn decline_reaches_the_initiator_as_permission_denied() {
        let (mut ci, mut cr) = socket_pair();
        let initiator = thread::spawn(move || pair_initiator(&mut ci, "Mac", |_| {}).unwrap_err());

        let decision = pair_responder(&mut cr, "s3cret", 5599, "PC", |_| false).unwrap();
        assert!(matches!(decision, PairDecision::Declined(_)));

        let err = initiator.join().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("declined"), "unexpected message: {err}");
    }

    #[test]
    fn hangup_before_the_answer_is_also_permission_denied() {
        // The receiver dying while its dialog is open must not surface as a
        // confusing EOF — from the user's side it simply was not confirmed.
        let (mut ci, mut cr) = socket_pair();
        let initiator = thread::spawn(move || pair_initiator(&mut ci, "Mac", |_| {}).unwrap_err());

        // Hand-roll a responder that goes silent instead of answering.
        let mut hs = snow::Builder::new(NOISE_PARAMS.parse().unwrap()).build_responder().unwrap();
        let mut buf = [0u8; MAX_HANDSHAKE];
        let n = read_frame(&mut cr, &mut buf).unwrap();
        let mut tmp = [0u8; MAX_HANDSHAKE];
        hs.read_message(&buf[..n], &mut tmp).unwrap();
        let n = hs.write_message(&[PAIRING_VERSION], &mut buf).unwrap();
        write_frame(&mut cr, &buf[..n]).unwrap();
        let mut t = hs.into_transport_mode().unwrap();
        let mut out = [0u8; MAX_MSG];
        recv_msg(&mut t, &mut cr, &mut out).unwrap(); // HELLO, then hang up
        drop(cr);

        let err = initiator.join().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn version_mismatch_is_named_on_both_sides() {
        let (mut ci, mut cr) = socket_pair();
        // Impersonate a future sender: same pattern, different version byte.
        let initiator = thread::spawn(move || {
            let mut hs = snow::Builder::new(NOISE_PARAMS.parse().unwrap())
                .build_initiator()
                .unwrap();
            let mut buf = [0u8; MAX_HANDSHAKE];
            let n = hs.write_message(&[PAIRING_VERSION + 1], &mut buf).unwrap();
            write_frame(&mut ci, &buf[..n]).unwrap();
            // Read the reply so the responder's write cannot fail on a dead peer.
            let _ = read_frame(&mut ci, &mut buf);
        });

        let err = pair_responder(&mut cr, "s3cret", 5599, "PC", |_| {
            panic!("must not prompt the user on a version mismatch")
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("pairing version mismatch"), "got: {err}");
        initiator.join().unwrap();
    }

    #[test]
    fn code_is_six_digits_and_differs_between_sessions() {
        let run = || {
            let (mut ci, mut cr) = socket_pair();
            let initiator =
                thread::spawn(move || pair_initiator(&mut ci, "Mac", |_| {}).unwrap().code);
            pair_responder(&mut cr, "s3cret", 5599, "PC", |_| true).unwrap();
            initiator.join().unwrap()
        };
        let a = run();
        let b = run();
        for c in [&a, &b] {
            assert_eq!(c.len(), 6, "code must be 6 digits: {c}");
            assert!(c.bytes().all(|b| b.is_ascii_digit()), "non-digit in code: {c}");
        }
        // Ephemeral keys are fresh per session, so a replayed code is worthless.
        // This is what stops a relayed handshake from matching.
        assert_ne!(a, b, "two sessions produced the same code");
        assert_eq!(code_display("482913"), "482 913");
    }

    #[test]
    fn peer_names_from_the_network_are_sanitized_before_display() {
        // A name is untrusted text headed straight for a dialog.
        assert_eq!(sanitize_name("Mac\n\rAllow: yes"), "MacAllow: yes");
        assert_eq!(sanitize_name("   "), "unknown device");
        assert_eq!(sanitize_name(""), "unknown device");
        let long = "é".repeat(100);
        let clamped = sanitize_name(&long);
        assert!(clamped.len() <= MAX_NAME_LEN);
        // Cut on a char boundary, never mid-codepoint.
        assert_eq!(clamped.chars().count(), MAX_NAME_LEN / 2);
    }

    #[test]
    fn a_truncated_result_is_rejected_rather_than_half_applied() {
        let (mut ci, mut cr) = socket_pair();
        let initiator = thread::spawn(move || pair_initiator(&mut ci, "Mac", |_| {}).unwrap_err());

        // Hand-roll a responder that claims success but sends no payload.
        let mut hs = snow::Builder::new(NOISE_PARAMS.parse().unwrap()).build_responder().unwrap();
        let mut buf = [0u8; MAX_HANDSHAKE];
        let n = read_frame(&mut cr, &mut buf).unwrap();
        let mut tmp = [0u8; MAX_HANDSHAKE];
        hs.read_message(&buf[..n], &mut tmp).unwrap();
        let n = hs.write_message(&[PAIRING_VERSION], &mut buf).unwrap();
        write_frame(&mut cr, &buf[..n]).unwrap();
        let mut t = hs.into_transport_mode().unwrap();
        let mut out = [0u8; MAX_MSG];
        recv_msg(&mut t, &mut cr, &mut out).unwrap(); // HELLO
        send_msg(&mut t, &mut cr, &[RESULT_ACCEPTED]).unwrap(); // ...and nothing else

        let err = initiator.join().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
