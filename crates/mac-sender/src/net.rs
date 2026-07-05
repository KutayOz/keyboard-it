//! Ortak ağ yardımcısı: win-receiver'a bağlan (kısa tekrar denemeli).

use std::io;
use std::net::TcpStream;
use std::thread::sleep;
use std::time::Duration;

/// Adrese bağlanmayı ~4 sn boyunca dener (win-receiver henüz ayakta olmayabilir).
pub fn connect_retry(addr: &str) -> io::Result<TcpStream> {
    let mut last_err = None;
    for _ in 0..40 {
        match TcpStream::connect(addr) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_millis(100));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "bağlanılamadı")))
}
