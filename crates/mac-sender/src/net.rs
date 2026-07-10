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
                // Sessiz kopmalarda sonsuz bloklamayı önle: el sıkışma yanıtı (read)
                // ve gönderim (write) en çok ~10 sn beklesin. Timeout'ta send/handshake
                // Err döner, çağıran bağlantıyı kapatıp yeniden dener (bulgu düzeltmesi).
                let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
                // Ölü peer algılama: TCP keepalive — win-receiver serve.rs ile AYNI
                // ayarlar (5 sn boşta + 3 sn aralıklı sonda). Mac uyur/Wi-Fi düşerse
                // (yarı-açık bağlantı, RST/EOF gelmez) gönderici de ~15 sn içinde
                // hata görüp reconnect'e düşer (protokol-ping'siz çözümün bu yarısı).
                {
                    use socket2::{SockRef, TcpKeepalive};
                    let ka = TcpKeepalive::new()
                        .with_time(Duration::from_secs(5))
                        .with_interval(Duration::from_secs(3));
                    let _ = SockRef::from(&s).set_tcp_keepalive(&ka);
                }
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
