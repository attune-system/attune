//! Origin-bound credentials for registry archive and Git sources.

use super::{validate_registry_headers, validate_remote_pack_url};
use crate::config::{RegistryIndexConfig, RegistrySourceHeaders};
use crate::{Error, Result};
use std::collections::HashMap;
use url::Url;

/// Validate an exact HTTPS origin and return its canonical spelling.
pub fn normalize_source_origin(origin: &str) -> Result<String> {
    let url = validate_remote_pack_url(origin)?;
    if origin.trim() != origin
        || origin.chars().any(char::is_control)
        || url.scheme() != "https"
        || url.path() != "/"
        || url.host_str().is_some_and(|host| host.contains('*'))
    {
        return Err(Error::validation(
            "Source header origins must be exact HTTPS origins without paths or wildcards",
        ));
    }
    canonical_origin(&url)
}

pub(super) fn normalize_source_headers(
    source_headers: RegistrySourceHeaders,
) -> Result<RegistrySourceHeaders> {
    let mut normalized = RegistrySourceHeaders::new();
    for (origin, headers) in source_headers {
        let origin = normalize_source_origin(&origin)?;
        validate_registry_headers(&headers)?;
        if normalized.insert(origin, headers).is_some() {
            return Err(Error::validation("Duplicate source header origin"));
        }
    }
    Ok(normalized)
}

pub(super) fn headers_for_source(
    registry: &RegistryIndexConfig,
    source_url: &Url,
) -> Result<HashMap<String, String>> {
    let source_origin = canonical_origin(source_url)?;
    let index_url = validate_remote_pack_url(&registry.url)?;
    let mut headers: HashMap<_, _> = if canonical_origin(&index_url)? == source_origin {
        registry
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect()
    } else {
        HashMap::new()
    };
    if let Some(overrides) = registry.source_headers.get(&source_origin) {
        headers.extend(
            overrides
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.clone())),
        );
    }
    Ok(headers)
}

fn canonical_origin(url: &Url) -> Result<String> {
    let mut url = url.clone();
    let host = url
        .host_str()
        .ok_or_else(|| Error::validation("Source URL must include a host"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    url.set_host(Some(&host))
        .map_err(|_| Error::validation("Source URL has an invalid host"))?;
    Ok(url.origin().ascii_serialization())
}

/// Keep secrets out of Git's argv and scope extra headers to the validated origin.
pub(super) fn apply_git_source_headers(
    command: &mut tokio::process::Command,
    url: &Url,
    headers: &HashMap<String, String>,
) -> Result<()> {
    validate_registry_headers(headers)?;
    let config_key = format!("http.{}/.extraHeader", canonical_origin(url)?);
    command.env("GIT_CONFIG_COUNT", headers.len().to_string());
    for (index, (name, value)) in headers.iter().enumerate() {
        command
            .env(format!("GIT_CONFIG_KEY_{index}"), &config_key)
            .env(
                format!("GIT_CONFIG_VALUE_{index}"),
                format!("{name}: {value}"),
            );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> RegistryIndexConfig {
        RegistryIndexConfig {
            url: "https://REGISTRY.example.com.:443/index.json".to_string(),
            priority: 0,
            enabled: true,
            name: None,
            headers: HashMap::from([
                (
                    "Authorization".to_string(),
                    "Bearer index-token".to_string(),
                ),
                ("X-Index".to_string(), "index-value".to_string()),
            ]),
            source_headers: HashMap::from([
                (
                    "https://registry.example.com".to_string(),
                    HashMap::from([
                        (
                            "authorization".to_string(),
                            "Bearer same-origin-token".to_string(),
                        ),
                        ("X-Source".to_string(), "same-origin-value".to_string()),
                    ]),
                ),
                (
                    "https://git.example.com:8443".to_string(),
                    HashMap::from([
                        ("Authorization".to_string(), "Basic git-token".to_string()),
                        ("X-Source".to_string(), "git-value".to_string()),
                    ]),
                ),
            ]),
        }
    }

    #[test]
    fn source_header_origins_require_exact_clean_https_origins() {
        for origin in [
            "http://source.example.com",
            "https://source.example.com/path",
            "https://*.example.com",
            "https://source.example.com?token=secret",
            "https://source.example.com#fragment",
            "https://user:secret@source.example.com",
            " https://source.example.com",
            "file:///tmp",
            "null",
        ] {
            assert!(normalize_source_origin(origin).is_err());
        }
        assert_eq!(
            normalize_source_origin("HTTPS://SOURCE.EXAMPLE.COM.:443/").unwrap(),
            "https://source.example.com"
        );
        assert_eq!(
            normalize_source_origin("https://source.example.com:8443").unwrap(),
            "https://source.example.com:8443"
        );
        assert_eq!(
            normalize_source_origin("https://[::1]:8443/").unwrap(),
            "https://[::1]:8443"
        );
    }

    #[test]
    fn same_origin_sources_inherit_and_override_headers_case_insensitively() {
        let headers = headers_for_source(
            &registry(),
            &Url::parse("https://registry.example.com:443/team/pack.git").unwrap(),
        )
        .unwrap();
        assert_eq!(headers.len(), 3);
        assert_eq!(headers["authorization"], "Bearer same-origin-token");
        assert_eq!(headers["x-index"], "index-value");
        assert_eq!(headers["x-source"], "same-origin-value");
    }

    #[test]
    fn other_origins_receive_only_explicit_source_headers() {
        let headers = headers_for_source(
            &registry(),
            &Url::parse("https://git.example.com:8443/team/pack.git").unwrap(),
        )
        .unwrap();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers["authorization"], "Basic git-token");
        assert_eq!(headers["x-source"], "git-value");
        assert!(!headers.contains_key("x-index"));
    }

    #[test]
    fn unlisted_origins_ports_subdomains_and_schemes_receive_no_headers() {
        for source in [
            "https://other.example.com/pack.zip",
            "https://git.example.com/pack.git",
            "https://registry.example.com:8443/pack.zip",
            "http://registry.example.com/pack.zip",
            "https://sub.registry.example.com/pack.zip",
            "https://registry.example.com.evil.test/pack.zip",
        ] {
            assert!(
                headers_for_source(&registry(), &Url::parse(source).unwrap())
                    .unwrap()
                    .is_empty(),
                "{source}"
            );
        }
    }

    #[test]
    fn duplicate_normalized_origins_and_unsafe_headers_are_rejected() {
        assert!(normalize_source_headers(HashMap::from([
            ("https://example.com".to_string(), HashMap::new()),
            ("https://EXAMPLE.com:443/".to_string(), HashMap::new()),
        ]))
        .is_err());
        assert!(normalize_source_headers(HashMap::from([(
            "https://example.com".to_string(),
            HashMap::from([("Host".to_string(), "other.example.com".to_string())])
        ),]))
        .is_err());
    }

    #[tokio::test]
    async fn git_consumes_secret_headers_from_environment_only_for_the_exact_origin() {
        let registry = registry();
        let url = Url::parse("https://git.example.com:8443/team/pack.git").unwrap();
        let headers = headers_for_source(&registry, &url).unwrap();
        for (requested, expected) in [
            ("https://git.example.com:8443/team/pack.git", true),
            ("https://git.example.com/team/pack.git", false),
            ("http://git.example.com:8443/team/pack.git", false),
            ("https://other.example.com:8443/team/pack.git", false),
        ] {
            let mut command = tokio::process::Command::new("git");
            crate::child_process_environment::ChildProcessEnvironment::default()
                .apply(command.as_std_mut());
            command
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["config", "--get-urlmatch", "http.extraHeader", requested]);
            apply_git_source_headers(&mut command, &url, &headers).unwrap();
            assert!(!command
                .as_std()
                .get_args()
                .any(|argument| argument.to_string_lossy().contains("git-token")));
            let values: Vec<_> = command
                .as_std()
                .get_envs()
                .filter_map(|(_, value)| value)
                .collect();
            assert!(values.contains(&std::ffi::OsStr::new("authorization: Basic git-token")));
            assert!(values.contains(&std::ffi::OsStr::new("x-source: git-value")));
            let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(output.status.success(), expected);
            if expected {
                let values = String::from_utf8(output.stdout).unwrap();
                assert!(headers
                    .iter()
                    .any(|(name, value)| values.trim() == format!("{name}: {value}")));
                assert!(!values.contains("index-token"));
            } else {
                assert!(output.stdout.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn git_transport_sends_all_configured_source_headers_without_argv_secrets() {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        // Exercise Git's HTTP header mechanism locally; registry Git URL admission still requires HTTPS.
        let url = Url::parse(&format!(
            "http://{}/pack.git",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                    assert!(request.len() < 16 * 1024);
                }
                stream
                    .write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                String::from_utf8(request).unwrap().to_ascii_lowercase()
            })
            .await
            .unwrap()
        });
        let mut command = tokio::process::Command::new("git");
        crate::child_process_environment::ChildProcessEnvironment::default()
            .apply(command.as_std_mut());
        command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["ls-remote", url.as_str()])
            .kill_on_drop(true);
        let headers = HashMap::from([
            ("Authorization".to_string(), "Basic git-token".to_string()),
            ("X-Source".to_string(), "git-value".to_string()),
        ]);
        apply_git_source_headers(&mut command, &url, &headers).unwrap();
        assert!(!command
            .as_std()
            .get_args()
            .any(|arg| arg.to_string_lossy().contains("git-token")));
        let output = tokio::time::timeout(Duration::from_secs(5), command.output()).await;
        let request = server.await.unwrap();
        assert!(!output.unwrap().unwrap().status.success());
        assert!(request.contains("authorization: basic git-token\r\n"));
        assert!(request.contains("x-source: git-value\r\n"));
    }
}
