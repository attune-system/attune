//! Destination admission lives at DNS resolution, so the connector receives only
//! checked addresses, never a hostname to resolve a second time.
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::Url;
use std::net::{IpAddr, SocketAddr};

pub(super) fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !matches!(a, 0 | 10 | 127 | 224..=255)
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 169 && b == 254)
                && !(a == 172 && (16..=31).contains(&b))
                && !(a == 192 && (b == 168 || (b == 0 && c <= 2) || (b == 88 && c == 99)))
                && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                && !(a == 203 && b == 0 && c == 113)
                // Azure's platform virtual IP is public-looking but exposes
                // host/platform services. Other common metadata IPs fall in
                // link-local, private, CGNAT or unique-local ranges above/below.
                && ip.octets() != [168, 63, 129, 16]
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_address(IpAddr::V4(v4));
            }
            let s = ip.segments();
            // Admit global unicast only; exclude special-purpose protocol,
            // documentation and 6to4 ranges (embedded IPv4 routing bypass).
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

pub(super) fn check_literal(url: &Url) -> Result<(), &'static str> {
    let ip = url.host_str().and_then(|host| {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .ok()
    });
    if ip.is_some_and(|ip| !public_address(ip)) {
        return Err("HTTP destination is not public");
    }
    Ok(())
}

pub(super) fn checked_addresses(addresses: Vec<SocketAddr>) -> std::io::Result<Addrs> {
    // Fail closed for mixed public/private answers, not just the first answer.
    if addresses.is_empty() || addresses.iter().any(|addr| !public_address(addr.ip())) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "HTTP destination is not public",
        ));
    }
    Ok(Box::new(addresses.into_iter()))
}

pub(super) struct PublicResolver;
impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            Ok(checked_addresses(addresses)?)
        })
    }
}
