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
    BadTag(u8),
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

// ===== Birleşik giriş olayı: KeyEvent'i sarar + fare varyantları =====

/// Tel üzerindeki InputEvent varyantını belirleyen etiket baytı.
pub mod tag {
    pub const KEY: u8 = 0;
    pub const MOUSE_MOVE: u8 = 1;
    pub const MOUSE_BUTTON: u8 = 2;
    pub const SCROLL: u8 = 3;
}

/// Fare butonu kimliği (küçük tam sayı; tel ve her iki taraf için ortak).
pub mod mousebtn {
    pub const LEFT: u8 = 0;
    pub const RIGHT: u8 = 1;
    pub const MIDDLE: u8 = 2;
}

/// Birleşik giriş olayı. Mevcut KeyEvent aynen sarılır; fare varyantları eklenir.
/// Hareket RELATİFtir (delta): mutlak Mac koordinatları Windows'ta anlamsız.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputEvent {
    Key(KeyEvent),
    MouseMove { dx: i16, dy: i16 },
    MouseButton { button: u8, down: bool }, // button: mousebtn::*
    Scroll { dx: i8, dy: i8 },              // dy: dikey, dx: yatay (tick sayısı)
}

/// En büyük olası kodlama: 1 etiket baytı + 5 baytlık Key payload'u.
pub const INPUT_MAX_LEN: usize = 1 + WIRE_LEN; // = 6

impl InputEvent {
    /// Sabit tampona kodla; (buf, kullanılan_uzunluk) döner. Heap yok.
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
                b[1] = *dx as u8; // i8 -> u8 bit-koruyan
                b[2] = *dy as u8;
                3
            }
        };
        (b, len)
    }

    /// İlk etiket baytından çöz; her varyant kendi uzunluğunu doğrular.
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
