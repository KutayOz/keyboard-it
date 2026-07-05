//! Paylaşılan tel formatı (wire format) — mac-sender ve win-receiver'ın ORTAK dili.
//!
//! Gerçek "köprü" budur: tuş olayının nasıl kodlandığı bir kez burada tanımlanır,
//! iki taraf da aynı `KeyEvent` tipini `use` eder. Böylece protokol anlaşmazlığı
//! derleme zamanında imkansız hale gelir.

/// LAN'de tuş vuruşlarını şifreleyen Noise (NNpsk0) katmanı.
pub mod secure;

/// Mesaj türü. Tek bir u8 etiketi olarak kodlanır.
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

/// Sabit 5 baytlık tel kaydı. Yerleşim (big-endian = ağ sırası):
///   [0]      mesaj türü (u8)
///   [1..=2]  HID usage  (u16, USB HID Usage ID, Usage Page 0x07 — OS'tan bağımsız)
///   [3..=4]  modifiers  (u16, bit maskesi: L/R Ctrl/Shift/Alt/GUI)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyEvent {
    pub msg: MsgType,
    pub hid_usage: u16,
    pub modifiers: u16,
}

/// Bir olayın tel üzerindeki sabit uzunluğu.
pub const WIRE_LEN: usize = 5;

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    ShortBuffer,
    BadMsgType(u8),
}

impl KeyEvent {
    /// Sabit 5 baytlık diziye kodla. Ayırma yok, serializer yok, deterministik genişlik.
    pub fn encode(&self) -> [u8; WIRE_LEN] {
        let u = self.hid_usage.to_be_bytes();
        let m = self.modifiers.to_be_bytes();
        [self.msg as u8, u[0], u[1], m[0], m[1]]
    }

    /// `buf`'ın ilk tam 5 baytından çöz.
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

/// Modifier bit maskesi (USB HID boot-keyboard modifier byte'ıyla eşleşir, u16'ya genişletildi).
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

/// mac-sender'ın bağlanacağı, win-receiver'ın dinleyeceği varsayılan TCP portu.
pub const DEFAULT_PORT: u16 = 5599;

use std::io::{self, Read, Write};

impl KeyEvent {
    /// Olayı "4 baytlık big-endian uzunluk öneki + payload" olarak yaz (framed).
    /// Framing sabit-5-bayt için şart değil ama ileride alanlar eklenirse tel formatı
    /// kırılmadan büyüsün diye baştan koyuyoruz.
    pub fn write_framed<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let payload = self.encode();
        w.write_all(&(payload.len() as u32).to_be_bytes())?;
        w.write_all(&payload)?;
        w.flush()
    }

    /// Bir framed olayı oku (uzunluk öneki + payload). Bağlantı kapanırsa
    /// `UnexpectedEof` döner — çağıran bunu "karşı taraf gitti" olarak yorumlar.
    pub fn read_framed<R: Read>(r: &mut R) -> io::Result<KeyEvent> {
        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len == 0 || len > 64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "geçersiz çerçeve boyu"));
        }
        let mut payload = [0u8; 64];
        r.read_exact(&mut payload[..len])?;
        KeyEvent::decode(&payload[..len])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("çözme hatası: {:?}", e)))
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
    fn framed_roundtrip() {
        use std::io::Cursor;
        let e = KeyEvent { msg: MsgType::Up, hid_usage: 0x0B /* 'h' */, modifiers: 0 };
        let mut buf = Vec::new();
        e.write_framed(&mut buf).unwrap();
        assert_eq!(buf.len(), 4 + WIRE_LEN); // uzunluk öneki + payload
        let mut cur = Cursor::new(buf);
        assert_eq!(KeyEvent::read_framed(&mut cur).unwrap(), e);
    }
}
