//! Webhook security helpers for HMAC verification and validation

use hmac::{digest::KeyInit, Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

/// Verify HMAC signature for webhook payload
pub fn verify_hmac_signature(
    payload: &[u8],
    signature: &str,
    secret: &str,
    algorithm: &str,
) -> Result<bool, String> {
    // Parse signature format (e.g., "sha256=abc123..." or just "abc123...")
    let (algo_from_sig, hex_signature) = if signature.contains('=') {
        let parts: Vec<&str> = signature.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err("Invalid signature format".to_string());
        }
        (Some(parts[0]), parts[1])
    } else {
        (None, signature)
    };

    // Verify algorithm matches if specified in signature
    if let Some(sig_algo) = algo_from_sig {
        if sig_algo != algorithm {
            return Err(format!(
                "Algorithm mismatch: expected {}, got {}",
                algorithm, sig_algo
            ));
        }
    }

    // Decode hex signature
    let expected_signature =
        hex::decode(hex_signature).map_err(|e| format!("Invalid hex signature: {}", e))?;

    // Compute HMAC based on algorithm
    let is_valid = match algorithm {
        "sha256" => verify_hmac_sha256(payload, &expected_signature, secret),
        "sha512" => verify_hmac_sha512(payload, &expected_signature, secret),
        "sha1" => verify_hmac_sha1(payload, &expected_signature, secret),
        _ => return Err(format!("Unsupported algorithm: {}", algorithm)),
    };

    Ok(is_valid)
}

/// Verify HMAC-SHA256 signature
fn verify_hmac_sha256(payload: &[u8], expected: &[u8], secret: &str) -> bool {
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    mac.update(payload);

    // Use constant-time comparison
    mac.verify_slice(expected).is_ok()
}

/// Verify HMAC-SHA512 signature
fn verify_hmac_sha512(payload: &[u8], expected: &[u8], secret: &str) -> bool {
    type HmacSha512 = Hmac<Sha512>;

    let mut mac = match HmacSha512::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    mac.update(payload);

    mac.verify_slice(expected).is_ok()
}

/// Verify HMAC-SHA1 signature (legacy, not recommended)
fn verify_hmac_sha1(payload: &[u8], expected: &[u8], secret: &str) -> bool {
    type HmacSha1 = Hmac<Sha1>;

    let mut mac = match HmacSha1::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    mac.update(payload);

    mac.verify_slice(expected).is_ok()
}

/// Generate HMAC signature for testing
pub fn generate_hmac_signature(
    payload: &[u8],
    secret: &str,
    algorithm: &str,
) -> Result<String, String> {
    let signature = match algorithm {
        "sha256" => {
            type HmacSha256 = Hmac<Sha256>;
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .map_err(|e| format!("Invalid key length: {}", e))?;
            mac.update(payload);
            let result = mac.finalize();
            hex::encode(result.into_bytes())
        }
        "sha512" => {
            type HmacSha512 = Hmac<Sha512>;
            let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
                .map_err(|e| format!("Invalid key length: {}", e))?;
            mac.update(payload);
            let result = mac.finalize();
            hex::encode(result.into_bytes())
        }
        "sha1" => {
            type HmacSha1 = Hmac<Sha1>;
            let mut mac = HmacSha1::new_from_slice(secret.as_bytes())
                .map_err(|e| format!("Invalid key length: {}", e))?;
            mac.update(payload);
            let result = mac.finalize();
            hex::encode(result.into_bytes())
        }
        _ => return Err(format!("Unsupported algorithm: {}", algorithm)),
    };

    Ok(format!("{}={}", algorithm, signature))
}

/// Check if IP address matches a CIDR block
pub fn check_ip_in_cidr(ip: &str, cidr: &str) -> Result<bool, String> {
    use std::net::IpAddr;

    let ip_addr: IpAddr = ip
        .parse()
        .map_err(|e| format!("Invalid IP address: {}", e))?;

    // If CIDR doesn't contain '/', treat it as a single IP
    if !cidr.contains('/') {
        let cidr_addr: IpAddr = cidr
            .parse()
            .map_err(|e| format!("Invalid CIDR notation: {}", e))?;
        return Ok(ip_addr == cidr_addr);
    }

    // Parse CIDR notation
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return Err("Invalid CIDR format".to_string());
    }

    let network_addr: IpAddr = parts[0]
        .parse()
        .map_err(|e| format!("Invalid network address: {}", e))?;
    let prefix_len: u8 = parts[1]
        .parse()
        .map_err(|e| format!("Invalid prefix length: {}", e))?;

    // Validate the network independently of the sender family. A valid network
    // of the other family is a nonmatch; malformed prefixes remain errors.
    match network_addr {
        IpAddr::V4(_) if prefix_len > 32 => {
            return Err("IPv4 prefix length must be <= 32".to_string());
        }
        IpAddr::V6(_) if prefix_len > 128 => {
            return Err("IPv6 prefix length must be <= 128".to_string());
        }
        _ => {}
    }

    // Convert to bytes for comparison
    match (ip_addr, network_addr) {
        (IpAddr::V4(ip), IpAddr::V4(network)) => {
            let ip_bits = u32::from(ip);
            let network_bits = u32::from(network);
            let mask = if prefix_len == 0 {
                0
            } else {
                !0u32 << (32 - prefix_len)
            };
            Ok((ip_bits & mask) == (network_bits & mask))
        }
        (IpAddr::V6(ip), IpAddr::V6(network)) => {
            let ip_bits = u128::from(ip);
            let network_bits = u128::from(network);
            let mask = if prefix_len == 0 {
                0
            } else {
                !0u128 << (128 - prefix_len)
            };
            Ok((ip_bits & mask) == (network_bits & mask))
        }
        _ => Ok(false),
    }
}

/// Check if IP is in any of the CIDR blocks in the whitelist
pub fn check_ip_in_whitelist(ip: &str, whitelist: &[String]) -> Result<bool, String> {
    for cidr in whitelist {
        match check_ip_in_cidr(ip, cidr) {
            Ok(true) => return Ok(true),
            Ok(false) => continue,
            Err(e) => return Err(format!("Error checking CIDR {}: {}", cidr, e)),
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_verify_hmac_sha256() {
        let payload = b"test payload";
        let secret = "my-secret-key";
        let signature = generate_hmac_signature(payload, secret, "sha256").unwrap();

        assert!(verify_hmac_signature(payload, &signature, secret, "sha256").unwrap());
    }

    #[test]
    fn test_verify_hmac_wrong_secret() {
        let payload = b"test payload";
        let secret = "my-secret-key";
        let wrong_secret = "wrong-key";
        let signature = generate_hmac_signature(payload, secret, "sha256").unwrap();

        assert!(!verify_hmac_signature(payload, &signature, wrong_secret, "sha256").unwrap());
    }

    #[test]
    fn test_verify_hmac_wrong_payload() {
        let payload = b"test payload";
        let wrong_payload = b"wrong payload";
        let secret = "my-secret-key";
        let signature = generate_hmac_signature(payload, secret, "sha256").unwrap();

        assert!(!verify_hmac_signature(wrong_payload, &signature, secret, "sha256").unwrap());
    }

    #[test]
    fn test_verify_hmac_sha512() {
        let payload = b"test payload";
        let secret = "my-secret-key";
        let signature = generate_hmac_signature(payload, secret, "sha512").unwrap();

        assert!(verify_hmac_signature(payload, &signature, secret, "sha512").unwrap());
    }

    #[test]
    fn test_verify_hmac_without_algorithm_prefix() {
        let payload = b"test payload";
        let secret = "my-secret-key";
        let signature = generate_hmac_signature(payload, secret, "sha256").unwrap();

        // Remove the "sha256=" prefix
        let hex_only = signature.split('=').nth(1).unwrap();

        assert!(verify_hmac_signature(payload, hex_only, secret, "sha256").unwrap());
    }

    #[test]
    fn test_check_ip_in_cidr_single_ip() {
        assert!(check_ip_in_cidr("192.168.1.1", "192.168.1.1").unwrap());
        assert!(!check_ip_in_cidr("192.168.1.2", "192.168.1.1").unwrap());
    }

    #[test]
    fn test_check_ip_in_cidr_block() {
        assert!(check_ip_in_cidr("192.168.1.100", "192.168.1.0/24").unwrap());
        assert!(check_ip_in_cidr("192.168.1.1", "192.168.1.0/24").unwrap());
        assert!(check_ip_in_cidr("192.168.1.254", "192.168.1.0/24").unwrap());
        assert!(!check_ip_in_cidr("192.168.2.1", "192.168.1.0/24").unwrap());
    }

    #[test]
    fn test_check_ip_in_cidr_ipv6() {
        assert!(check_ip_in_cidr("2001:db8::1", "2001:db8::/32").unwrap());
        assert!(!check_ip_in_cidr("2001:db9::1", "2001:db8::/32").unwrap());
    }

    #[test]
    fn dual_stack_networks_are_nonmatches_but_malformed_prefixes_are_errors() {
        assert!(!check_ip_in_cidr("2001:db8::1", "192.0.2.0/24").unwrap());
        assert!(!check_ip_in_cidr("192.0.2.1", "2001:db8::/32").unwrap());
        assert!(check_ip_in_cidr("2001:db8::1", "192.0.2.0/33").is_err());
        assert!(check_ip_in_cidr("192.0.2.1", "2001:db8::/129").is_err());
        assert!(check_ip_in_cidr("2001:db8::1", "not-a-network").is_err());
    }

    #[test]
    fn test_check_ip_in_whitelist() {
        let whitelist = vec![
            "192.168.1.0/24".to_string(),
            "10.0.0.0/8".to_string(),
            "172.16.5.10".to_string(),
        ];

        assert!(check_ip_in_whitelist("192.168.1.100", &whitelist).unwrap());
        assert!(check_ip_in_whitelist("10.20.30.40", &whitelist).unwrap());
        assert!(check_ip_in_whitelist("172.16.5.10", &whitelist).unwrap());
        assert!(!check_ip_in_whitelist("8.8.8.8", &whitelist).unwrap());
    }
}

/// Resolve one canonical address. Only explicitly trusted socket peers may supply
/// X-Forwarded-For; walk from the nearest hop to the first untrusted address.
pub fn canonical_client_ip(
    peer: Option<std::net::IpAddr>,
    headers: &axum::http::HeaderMap,
    proxies: &[String],
) -> Result<std::net::IpAddr, String> {
    use std::net::IpAddr;
    fn normalize(ip: IpAddr) -> IpAddr {
        match ip {
            IpAddr::V6(ip) => ip
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(ip)),
            ip => ip,
        }
    }
    // Validate every network, even when a preceding entry matches.
    for network in proxies {
        let address = network.split('/').next().unwrap_or("");
        check_ip_in_cidr(address, network)?;
    }
    let trusted = |ip: IpAddr| {
        proxies
            .iter()
            .any(|network| check_ip_in_cidr(&ip.to_string(), network).unwrap_or(false))
    };
    let mut current = normalize(peer.ok_or("Missing socket peer")?);
    if !trusted(current) {
        return Ok(current);
    }
    let values: Vec<_> = headers.get_all("x-forwarded-for").iter().collect();
    if values.len() != 1 {
        return Err("Trusted proxy must supply one forwarding chain".into());
    }
    let chain = values[0].to_str().map_err(|_| "Invalid forwarding chain")?;
    let hops = chain
        .split(',')
        .map(|hop| hop.trim().parse::<IpAddr>().map(normalize))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid forwarding address")?;
    if hops.len() > 64 {
        return Err("Forwarding chain too long".into());
    }
    for hop in hops.into_iter().rev() {
        if !trusted(current) {
            break;
        }
        current = hop;
    }
    Ok(current)
}

/// Security configuration is a fail-closed contract, including disabled sections.
pub fn validate_receiver_security(config: Option<&serde_json::Value>) -> Result<(), String> {
    let Some(config) = config else {
        return Ok(());
    };
    let object = config.as_object().ok_or("Invalid webhook configuration")?;
    for name in object.keys() {
        if ![
            "hmac",
            "ip_whitelist",
            "rate_limit",
            "payload_size_limit_kb",
        ]
        .contains(&name.as_str())
        {
            return Err("Unknown webhook security section".into());
        }
    }
    if let Some(limit) = object.get("payload_size_limit_kb") {
        let limit = limit.as_i64().ok_or("Invalid payload size limit")?;
        if limit <= 0 || limit.checked_mul(1024).is_none() {
            return Err("Invalid payload size limit".into());
        }
    }
    for name in ["hmac", "ip_whitelist", "rate_limit"] {
        let Some(section) = object.get(name) else {
            continue;
        };
        let section = section.as_object().ok_or("Invalid security section")?;
        let enabled = section
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or("Invalid security enable flag")?;
        if !enabled {
            continue;
        }
        match name {
            "hmac" => {
                if section
                    .get("secret")
                    .and_then(|v| v.as_str())
                    .is_none_or(str::is_empty)
                    || !matches!(
                        section.get("algorithm").and_then(|v| v.as_str()),
                        Some("sha256" | "sha512" | "sha1")
                    )
                {
                    return Err("Invalid HMAC configuration".into());
                }
            }
            "ip_whitelist" => {
                let ips = section
                    .get("ips")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing IP whitelist")?;
                if ips.is_empty() {
                    return Err("Empty IP whitelist".into());
                }
                for ip in ips {
                    let network = ip.as_str().ok_or("Invalid IP network")?;
                    check_ip_in_cidr(network.split('/').next().unwrap_or(""), network)?;
                }
            }
            "rate_limit" => {
                for field in ["requests", "window_seconds"] {
                    let value = section
                        .get(field)
                        .and_then(|v| v.as_i64())
                        .ok_or("Invalid rate limit")?;
                    if value <= 0 || value > i32::MAX as i64 {
                        return Err("Invalid rate limit".into());
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use axum::http::HeaderMap;
    #[test]
    fn proxy_chain_stops_at_first_untrusted_hop() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "192.168.1.1, 203.0.113.9, 10.1.1.2".parse().unwrap(),
        );
        let proxies = vec!["10.0.0.0/8".into()];
        assert_eq!(
            canonical_client_ip(Some("10.1.1.1".parse().unwrap()), &headers, &proxies)
                .unwrap()
                .to_string(),
            "203.0.113.9"
        );
        assert_eq!(
            canonical_client_ip(Some("203.0.113.1".parse().unwrap()), &headers, &proxies)
                .unwrap()
                .to_string(),
            "203.0.113.1"
        );
        assert!(canonical_client_ip(None, &headers, &proxies).is_err());
        headers.insert("x-forwarded-for", "bad, 10.1.1.2".parse().unwrap());
        assert!(
            canonical_client_ip(Some("10.1.1.1".parse().unwrap()), &headers, &proxies).is_err()
        );
        assert!(canonical_client_ip(
            Some("203.0.113.1".parse().unwrap()),
            &headers,
            &["invalid".into()]
        )
        .is_err());
    }
}

/// Correlate requests without retaining bearer credentials in the audit trail.
pub fn credential_fingerprint(key: &str) -> String {
    use sha2::Digest;
    hex::encode(Sha256::digest(key.as_bytes()))
}
