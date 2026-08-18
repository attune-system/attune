//! Central outbound URL and connection policy for pack registry traffic.

use crate::config::PackRegistryConfig;
use crate::{Error, Result};
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::lookup_host;
use url::{Host, Url};

#[derive(Debug, Clone)]
pub struct OutboundUrlPolicy {
    public_hosts: HashSet<String>,
    private_hosts: HashSet<String>,
    private_cidrs: Vec<IpCidr>,
    allow_http: bool,
    connect_timeout: Duration,
    total_timeout: Duration,
}

pub struct ValidatedUrl {
    pub url: Url,
    pub client: reqwest::Client,
    pub addresses: Vec<SocketAddr>,
}

pub fn validate_remote_pack_url(raw_url: &str) -> Result<Url> {
    let url = Url::parse(raw_url).map_err(|_| Error::validation("Invalid outbound pack URL"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(Error::validation(
            "Outbound pack URLs must use HTTP or HTTPS",
        ));
    }
    if url.host_str().is_none() {
        return Err(Error::validation("Outbound pack URL is missing a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::validation(
            "Outbound pack URLs must not contain credentials",
        ));
    }
    if url.fragment().is_some() {
        return Err(Error::validation(
            "Outbound pack URLs must not contain fragments",
        ));
    }
    if url.query().is_some() {
        return Err(Error::validation(
            "Outbound pack URLs must not contain query parameters; use encrypted headers for credentials",
        ));
    }
    Ok(url)
}

impl OutboundUrlPolicy {
    pub fn from_config(config: &PackRegistryConfig) -> Result<Self> {
        let normalize = |hosts: &[String]| {
            hosts
                .iter()
                .map(|host| host.trim().trim_end_matches('.').to_ascii_lowercase())
                .filter(|host| !host.is_empty())
                .collect::<HashSet<_>>()
        };
        let private_cidrs = config
            .approved_private_cidrs
            .iter()
            .map(|cidr| IpCidr::parse(cidr))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            public_hosts: normalize(&config.approved_public_hosts),
            private_hosts: normalize(&config.approved_private_hosts),
            private_cidrs,
            allow_http: config.allow_http,
            connect_timeout: Duration::from_secs(config.connect_timeout),
            total_timeout: Duration::from_secs(config.timeout),
        })
    }

    pub async fn validate(&self, raw_url: &str) -> Result<ValidatedUrl> {
        let mut url = validate_remote_pack_url(raw_url)?;
        if url.scheme() != "https" && !(self.allow_http && url.scheme() == "http") {
            return Err(Error::validation("Outbound pack URLs must use HTTPS"));
        }

        let host = url
            .host_str()
            .ok_or_else(|| Error::validation("Outbound pack URL is missing a host"))?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        url.set_host(Some(&host))
            .map_err(|_| Error::validation("Outbound pack URL has an invalid host"))?;
        if url.port().is_some_and(|port| {
            (url.scheme() == "https" && port == 443) || (url.scheme() == "http" && port == 80)
        }) {
            url.set_port(None)
                .map_err(|_| Error::validation("Outbound pack URL has an invalid port"))?;
        }
        let public_approved = self.public_hosts.contains(&host);
        let private_host_approved = self.private_hosts.contains(&host);

        let port = url
            .port_or_known_default()
            .ok_or_else(|| Error::validation("Outbound pack URL has no usable port"))?;
        let addresses: Vec<SocketAddr> = match url.host() {
            Some(Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
            Some(Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
            Some(Host::Domain(_)) => lookup_host((host.as_str(), port))
                .await
                .map_err(|e| Error::validation(format!("Failed to resolve '{}': {}", host, e)))?
                .collect(),
            None => Vec::new(),
        };
        if addresses.is_empty() {
            return Err(Error::validation(format!(
                "Outbound host '{}' resolved to no addresses",
                host
            )));
        }

        self.validate_addresses(&host, public_approved, private_host_approved, &addresses)?;

        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(self.connect_timeout)
            .timeout(self.total_timeout)
            .resolve_to_addrs(&host, &addresses)
            .user_agent(format!("attune-pack-client/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::internal(format!("Failed to build outbound client: {}", e)))?;
        Ok(ValidatedUrl {
            url,
            client,
            addresses,
        })
    }

    fn validate_addresses(
        &self,
        host: &str,
        public_approved: bool,
        private_host_approved: bool,
        addresses: &[SocketAddr],
    ) -> Result<()> {
        let mut saw_public = false;
        let mut saw_special = false;
        for address in addresses {
            let ip = normalize_ip(address.ip());
            if is_public_ip(ip) {
                saw_public = true;
                if !public_approved {
                    return Err(Error::validation(format!(
                        "Public outbound host '{}' is not explicitly approved",
                        host
                    )));
                }
            } else {
                saw_special = true;
                let cidr_approved = self.private_cidrs.iter().any(|cidr| cidr.contains(ip));
                if !private_host_approved && !cidr_approved {
                    return Err(Error::validation(format!(
                        "Outbound host '{}' resolved to private/special address {} without private-network approval",
                        host, ip
                    )));
                }
            }
        }
        if saw_public && saw_special {
            return Err(Error::validation(format!(
                "Outbound host '{}' has mixed public and private/special DNS answers",
                host
            )));
        }
        Ok(())
    }
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 88 && c == 99)
                || (a == 192 && b == 168)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 224
                || [a, b, c, d] == [255, 255, 255, 255])
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || (s[0] & 0xffc0) == 0xfec0
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || (s[0] == 0x2001 && s[1] == 0x0002)
                || (s[0] == 0x2001 && s[1] == 0x0010)
                || (s[0] == 0x2001 && (s[1] & 0xfff0) == 0x0020)
                || s[0] == 0x2002
                || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0)
                || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0x0001)
                || (s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0)
                || (s[0] == 0x2001 && s[1] == 0))
        }
    }
}

#[derive(Debug, Clone)]
struct IpCidr {
    network: IpAddr,
    prefix: u8,
}

impl IpCidr {
    fn parse(raw: &str) -> Result<Self> {
        let (address, prefix) = raw.trim().split_once('/').ok_or_else(|| {
            Error::configuration(format!("Invalid private CIDR '{}': missing prefix", raw))
        })?;
        let network: IpAddr = address.parse().map_err(|_| {
            Error::configuration(format!("Invalid private CIDR '{}': bad address", raw))
        })?;
        let prefix: u8 = prefix.parse().map_err(|_| {
            Error::configuration(format!("Invalid private CIDR '{}': bad prefix", raw))
        })?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return Err(Error::configuration(format!(
                "Invalid private CIDR '{}': prefix too large",
                raw
            )));
        }
        Ok(Self { network, prefix })
    }

    fn contains(&self, candidate: IpAddr) -> bool {
        match (self.network, candidate) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => prefix_matches(
                u32::from(network) as u128,
                u32::from(candidate) as u128,
                self.prefix,
                32,
            ),
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                prefix_matches(u128::from(network), u128::from(candidate), self.prefix, 128)
            }
            _ => false,
        }
    }
}

fn prefix_matches(network: u128, candidate: u128, prefix: u8, width: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let width_mask = if width == 128 {
        u128::MAX
    } else {
        (1_u128 << width) - 1
    };
    let host_bits = width - prefix;
    let mask = if host_bits == 0 {
        width_mask
    } else {
        width_mask ^ ((1_u128 << host_bits) - 1)
    };
    (network & mask) == (candidate & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_special_and_mapped_addresses() {
        for ip in [
            "127.0.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public_ip(normalize_ip(ip.parse().unwrap())), "{}", ip);
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn private_cidr_matches_only_its_network() {
        let cidr = IpCidr::parse("10.20.0.0/16").unwrap();
        assert!(cidr.contains("10.20.4.5".parse().unwrap()));
        assert!(!cidr.contains("10.21.4.5".parse().unwrap()));
    }

    #[test]
    fn mixed_dns_answers_are_rejected_even_with_private_approval() {
        let policy = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_public_hosts: vec!["mixed.example".into()],
            approved_private_hosts: vec!["mixed.example".into()],
            ..Default::default()
        })
        .unwrap();
        let addresses = [
            "8.8.8.8:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        ];
        assert!(policy
            .validate_addresses("mixed.example", true, true, &addresses)
            .unwrap_err()
            .to_string()
            .contains("mixed"));
    }

    #[tokio::test]
    async fn validated_client_does_not_follow_redirects() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let policy = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_private_hosts: vec!["127.0.0.1".into()],
            allow_http: true,
            ..Default::default()
        })
        .unwrap();
        let validated = policy
            .validate(&format!("http://{}/index.json", address))
            .await
            .unwrap();
        let response = validated.client.get(validated.url).send().await.unwrap();
        let status = response.status();

        match tokio::time::timeout(Duration::from_secs(1), &mut server_task).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                server_task.abort();
                let _ = server_task.await;
                panic!("mock redirect server did not stop within the timeout");
            }
        }

        assert_eq!(status, reqwest::StatusCode::FOUND);
    }

    #[tokio::test]
    async fn requires_approved_hosts_and_rejects_fragments_and_credentials() {
        let policy = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_public_hosts: vec!["example.com".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(policy
            .validate("https://unapproved.example/index.json")
            .await
            .is_err());
        assert!(policy
            .validate("https://user@example.com/index.json")
            .await
            .is_err());
        assert!(policy
            .validate("https://example.com/index.json#secret")
            .await
            .is_err());
        let query_error = match policy
            .validate("https://example.com/index.json?token=super-secret")
            .await
        {
            Ok(_) => panic!("query-bearing URL was accepted"),
            Err(error) => error,
        };
        assert!(!query_error.to_string().contains("super-secret"));
        assert!(policy
            .validate("https://example.com/index.json?")
            .await
            .is_err());
        assert!(policy.validate("file:///tmp/index.json").await.is_err());
    }

    #[tokio::test]
    async fn private_literal_needs_private_approval() {
        let denied = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_public_hosts: vec!["127.0.0.1".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(denied
            .validate("https://127.0.0.1/index.json")
            .await
            .is_err());

        let allowed = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_private_hosts: vec!["127.0.0.1".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(allowed
            .validate("https://127.0.0.1/index.json")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn normalized_url_matches_the_dns_override_host() {
        let policy = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_private_hosts: vec!["localhost".into()],
            allow_http: true,
            ..Default::default()
        })
        .unwrap();

        let validated = policy
            .validate("HTTP://LOCALHOST.:80/index.json")
            .await
            .unwrap();
        assert_eq!(validated.url.as_str(), "http://localhost/index.json");
        assert!(!validated.addresses.is_empty());
    }

    #[test]
    fn private_cidr_can_approve_a_host_without_private_host_approval() {
        let policy = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_private_cidrs: vec!["10.20.0.0/16".into()],
            ..Default::default()
        })
        .unwrap();
        let addresses = ["10.20.4.5:443".parse().unwrap()];
        policy
            .validate_addresses("internal.example", false, false, &addresses)
            .unwrap();
    }

    #[test]
    fn private_host_approval_does_not_approve_public_addresses() {
        let policy = OutboundUrlPolicy::from_config(&PackRegistryConfig {
            approved_private_hosts: vec!["misconfigured.example".into()],
            ..Default::default()
        })
        .unwrap();
        let addresses = ["8.8.8.8:443".parse().unwrap()];
        assert!(policy
            .validate_addresses("misconfigured.example", false, true, &addresses)
            .is_err());
    }
}
