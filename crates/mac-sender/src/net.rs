//! Shared network helper: connect to win-receiver (with short retries).

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How long to keep retrying one address before giving up on it.
const BUDGET: Duration = Duration::from_secs(4);

/// Per-address connect timeout.
///
/// This is the whole reason `connect_once` exists rather than
/// `TcpStream::connect(addr)`. A ".local" name routinely resolves to several
/// addresses, and `TcpStream::connect(&str)` walks them serially with no
/// per-address bound — so a single address that black-holes (SYN sent, nothing
/// comes back, no RST) costs macOS's default connect timeout of roughly 75
/// seconds before the next candidate is even tried.
///
/// That is not hypothetical: a Windows PC advertises its link-local IPv6
/// address over mDNS, the receiver binds IPv4, and the firewall drops rather
/// than rejects. The name resolved to `::1` (refused instantly), then that
/// link-local address (75 s of nothing), then the working IPv4 address. BUDGET
/// below cannot save it, because it is only checked BETWEEN attempts — the app
/// sat on "Connecting…" indefinitely with a reachable PC one entry down the list.
const PER_ADDR: Duration = Duration::from_secs(2);

/// Tries to connect to the address for ~4 s (win-receiver may not be up yet).
///
/// Bounded by wall clock, not attempt count: `peer_host` is normally a ".local"
/// name, and an unresolvable one costs ~5 s per attempt. Forty of those would
/// stall the caller for three minutes before it could try the fallback address —
/// long enough that a PC which changed IP would look permanently dead.
pub fn connect_retry(addr: &str) -> io::Result<TcpStream> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let err = match connect_once(addr) {
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
            Err(e) => e,
        };
        // Checked after the attempt, so a slow-failing address is still tried once.
        if Instant::now() >= deadline {
            return Err(err);
        }
        sleep(Duration::from_millis(100));
    }
}

/// One pass over every address `addr` resolves to, each with its own timeout.
///
/// Returns the first connection that comes up, or the last error if none do.
fn connect_once(addr: &str) -> io::Result<TcpStream> {
    let mut addrs: Vec<SocketAddr> = addr.to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("{addr} did not resolve to any address"),
        ));
    }
    addrs.sort_by_key(order);

    let mut last = None;
    for a in &addrs {
        match TcpStream::connect_timeout(a, PER_ADDR) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    // Unreachable in practice (the list is non-empty), but do not unwrap on it.
    Err(last.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrNotAvailable, format!("no address for {addr}"))
    }))
}

/// Try-order within one host. `sort_by_key` is stable, so addresses that tie
/// keep the resolver's own preference.
///
/// IPv4 first is not an opinion about IPv6 — it is that this app talks to a
/// win-receiver bound to an IPv4 socket, so the IPv4 address is the one that
/// can actually answer. Link-local and loopback go last because they are the
/// two that reliably waste PER_ADDR when a PC advertises more than it serves.
fn order(a: &SocketAddr) -> u8 {
    match a {
        SocketAddr::V4(_) => 0,
        SocketAddr::V6(v) if v.ip().is_loopback() => 3,
        // fe80::/10 — advertised by mDNS, rarely reachable for this.
        SocketAddr::V6(v) if v.ip().segments()[0] & 0xffc0 == 0xfe80 => 2,
        SocketAddr::V6(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    fn v6(s: &str) -> SocketAddr {
        SocketAddr::V6(SocketAddrV6::new(s.parse::<Ipv6Addr>().unwrap(), 5599, 0, 0))
    }
    fn v4(s: &str) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(s.parse::<Ipv4Addr>().unwrap(), 5599))
    }

    /// The exact list a PC's ".local" name resolved to when the app hung on
    /// "Connecting…": loopback first, then a black-holing link-local, with the
    /// only address that answers third.
    #[test]
    fn the_address_that_answers_is_tried_first() {
        let mut addrs = vec![
            v6("::1"),
            v6("fe80::f9b9:dd17:228f:cff2"),
            v4("192.168.68.55"),
            v6("2001:db8::1"),
        ];
        addrs.sort_by_key(order);
        assert_eq!(addrs[0], v4("192.168.68.55"), "IPv4 must come first");
        assert_eq!(addrs[3], v6("::1"), "loopback is the last thing worth trying");
        // The two that cost a full PER_ADDR each sit behind both usable ones.
        assert_eq!(addrs[1], v6("2001:db8::1"));
        assert_eq!(addrs[2], v6("fe80::f9b9:dd17:228f:cff2"));
    }

    /// Stability matters: with nothing to choose between two addresses, the
    /// resolver's order is the better guess than ours.
    #[test]
    fn equal_ranks_keep_resolver_order() {
        let mut addrs = vec![v4("10.0.0.2"), v4("10.0.0.1")];
        addrs.sort_by_key(order);
        assert_eq!(addrs, vec![v4("10.0.0.2"), v4("10.0.0.1")]);
    }

    #[test]
    fn a_name_that_does_not_resolve_is_an_error_not_a_hang() {
        // The point is that it RETURNS rather than blocking; the exact kind
        // varies by resolver, so do not pin it.
        let started = Instant::now();
        assert!(connect_once("no-such-host.invalid:5599").is_err());
        assert!(started.elapsed() < Duration::from_secs(30), "resolution should fail, not hang");
    }
}
