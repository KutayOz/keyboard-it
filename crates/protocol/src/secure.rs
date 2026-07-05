//! Noise_NNpsk0 üzerinden şifreli + karşılıklı-doğrulanmış taşıma katmanı.
//!
//! Mevcut "4-bayt big-endian uzunluk öneki + payload" framing'i AYNEN korunur;
//! çerçevenin payload'u artık düz `KeyEvent::encode()` yerine Noise ciphertext'idir.
//! NNpsk0: static key yok — kimlik doğrulama tamamen "iki taraf da aynı PSK'yi
//! biliyor" ilkesine dayanır. PSK'yi bilmeyen bir LAN dinleyicisi ne el sıkışmayı
//! tamamlayabilir ne de tek bir tuş vuruşunu okuyabilir.

use std::io::{self, Read, Write};

use crate::{InputEvent, INPUT_MAX_LEN};

const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const MAX_FRAME: usize = 65535; // Noise mesaj tavanı (snow::constants::MAXMSGLEN)

fn noise_err(e: snow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("noise: {e:?}"))
}

/// Kullanıcının parolasını (herhangi uzunluk) sabit 32 baytlık PSK'ye BLAKE2s-256 ile
/// sıkıştır. Alan-ayrımı öneki, aynı parolanın başka yerde aynı PSK'yi türetmesini önler.
pub fn psk_from_secret(secret: &str) -> [u8; 32] {
    use blake2::{Blake2s256, Digest};
    let mut h = Blake2s256::new();
    h.update(b"keyboard-it psk v1\0");
    h.update(secret.as_bytes());
    let mut psk = [0u8; 32];
    psk.copy_from_slice(&h.finalize());
    psk
}

/// KEYBOARD_IT_KEY'i oku ve PSK türet. Eksik/boşsa net hata (iki binary'de de aynı).
pub fn psk_from_env() -> io::Result<[u8; 32]> {
    match std::env::var("KEYBOARD_IT_KEY") {
        Ok(s) if !s.is_empty() => Ok(psk_from_secret(&s)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "KEYBOARD_IT_KEY ayarlı değil (iki tarafta da AYNI değeri ver)",
        )),
    }
}

// --- Mevcut 4-bayt BE uzunluk framing'i, ciphertext için aynen yeniden kullanılıyor ---
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
        return Err(io::Error::new(io::ErrorKind::InvalidData, "geçersiz çerçeve boyu"));
    }
    r.read_exact(&mut buf[..n])?;
    Ok(n)
}

/// mac-sender tarafı (TCP client = Noise initiator).
/// NNpsk0 = 2 mesaj:  -> e, psk   sonra   <- e, ee.
/// TCP connect'ten HEMEN sonra, herhangi bir KeyEvent'ten ÖNCE bir kez çağır.
pub fn handshake_initiator<S: Read + Write>(
    s: &mut S,
    psk: &[u8; 32],
) -> io::Result<snow::TransportState> {
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
        .psk(0, psk) // psk0: PSK ilk mesajdan ÖNCE karıştırılır
        .map_err(noise_err)?
        .build_initiator()
        .map_err(noise_err)?;
    let mut buf = [0u8; MAX_FRAME];
    // -> e, psk  (mesaj 1)
    let n = hs.write_message(&[], &mut buf).map_err(noise_err)?;
    write_frame(s, &buf[..n])?;
    // <- e, ee   (mesaj 2)  — bunu okumadan into_transport_mode() PANIKLER
    let n = read_frame(s, &mut buf)?;
    let mut tmp = [0u8; MAX_FRAME];
    hs.read_message(&buf[..n], &mut tmp).map_err(noise_err)?;
    hs.into_transport_mode().map_err(noise_err)
}

/// win-receiver tarafı (TCP server = Noise responder).
/// TCP accept'ten HEMEN sonra, okuma döngüsünden ÖNCE bir kez çağır.
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
    // <- e, psk  (mesaj 1) — yanlış PSK ise burada Decrypt hatası verir (fail-closed)
    let n = read_frame(s, &mut buf)?;
    let mut tmp = [0u8; MAX_FRAME];
    hs.read_message(&buf[..n], &mut tmp).map_err(noise_err)?;
    // -> e, ee   (mesaj 2)
    let n = hs.write_message(&[], &mut buf).map_err(noise_err)?;
    write_frame(s, &buf[..n])?;
    hs.into_transport_mode().map_err(noise_err)
}

/// Şifreli gönderim: InputEvent'i (Key veya fare) kodla, şifrele, çerçevele.
pub fn send_event<S: Write>(
    t: &mut snow::TransportState,
    s: &mut S,
    ev: &InputEvent,
) -> io::Result<()> {
    let (plain, plen) = ev.encode(); // ([u8; INPUT_MAX_LEN], usize)
    let mut ct = [0u8; INPUT_MAX_LEN + 16]; // düz metin + 16 baytlık Poly1305 tag
    let n = t.write_message(&plain[..plen], &mut ct).map_err(noise_err)?;
    write_frame(s, &ct[..n])
}

/// Şifreli alım. Nonce = Noise transport sayacı: snow sırasız/tekrar çerçeveleri
/// otomatik reddeder (replay koruması bedava). Kapanırsa read_exact UnexpectedEof döner.
/// Uzunluk doğrulaması artık varyant-bazlı InputEvent::decode içinde.
pub fn recv_event<S: Read>(
    t: &mut snow::TransportState,
    s: &mut S,
) -> io::Result<InputEvent> {
    let mut frame = [0u8; INPUT_MAX_LEN + 16];
    let n = read_frame(s, &mut frame)?;
    let mut plain = [0u8; INPUT_MAX_LEN + 16];
    let m = t.read_message(&frame[..n], &mut plain).map_err(noise_err)?;
    InputEvent::decode(&plain[..m])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("çözme hatası: {e:?}")))
}
