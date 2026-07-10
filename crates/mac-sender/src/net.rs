//! Shared network helper: connect to win-receiver (with short retries).

use std::io;
use std::net::TcpStream;
use std::thread::sleep;
use std::time::Duration;

/// Tries to connect to the address for ~4 s (win-receiver may not be up yet).
pub fn connect_retry(addr: &str) -> io::Result<TcpStream> {
    let mut last_err = None;
    for _ in 0..40 {
        match TcpStream::connect(addr) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                // Avoid blocking forever on silent drops: cap the handshake response
                // (read) and sends (write) at ~10 s. On timeout, send/handshake return
                // Err and the caller closes the connection and retries.
                let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
                // Dead-peer detection: TCP keepalive with the SAME settings as
                // win-receiver serve.rs (5 s idle + probes every 3 s). If the Mac sleeps
                // or Wi-Fi drops (half-open connection, no RST/EOF arrives), the sender
                // also sees an error within ~15 s and falls back to reconnecting (this
                // is the sender half of the no-protocol-ping solution).
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
    Err(last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "connect failed")))
}
