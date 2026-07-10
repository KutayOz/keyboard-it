//! Host identity for the settings window and the mDNS advertiser: computer name and
//! LAN IPv4 addresses. std-only on purpose — the manifest is frozen, so no
//! interface-enumeration crate; getaddrinfo on the own hostname covers the normal
//! case and a routing-table probe covers the rest.

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs, UdpSocket};

/// Windows computer name. The env var is set by the OS for every process, so this
/// never touches the registry or WinAPI.
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "windows-pc".to_string())
}

/// Best-effort LAN IPv4 addresses, shown in the GUI so the user can read the address
/// aloud when mDNS discovery fails. Resolving our own hostname via getaddrinfo returns
/// the addresses of every active adapter on Windows.
pub fn lan_ipv4s() -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    if let Ok(addrs) = (hostname().as_str(), 0u16).to_socket_addrs() {
        for addr in addrs {
            if let IpAddr::V4(v4) = addr.ip() {
                if usable(v4) && !out.contains(&v4) {
                    out.push(v4);
                }
            }
        }
    }
    // Fallback when name resolution yields nothing: `connect` on UDP sends no packet,
    // it only makes the OS pick the outbound interface — its source address is the
    // primary LAN IP.
    if out.is_empty() {
        if let Ok(sock) = UdpSocket::bind(("0.0.0.0", 0)) {
            if sock.connect(("8.8.8.8", 53)).is_ok() {
                if let Ok(local) = sock.local_addr() {
                    if let IpAddr::V4(v4) = local.ip() {
                        if usable(v4) {
                            out.push(v4);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Loopback and 169.254.x.x (self-assigned, i.e. no DHCP) would only mislead the user.
fn usable(ip: Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified()
}
