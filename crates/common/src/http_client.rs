//! HTTP clients for injected workers may run without a system CA bundle.

use std::error::Error as StdError;

/// Keep platform trust, including private CAs. Only an empty platform trust
/// store permits a retry with the bundled Mozilla roots. Recreate the same
/// builder so the retry retains the caller's timeout and other HTTP settings.
pub(crate) fn build_http_client(
    builder: impl Fn() -> reqwest::ClientBuilder,
) -> Result<reqwest::Client, reqwest::Error> {
    match builder().build() {
        Ok(client) => Ok(client),
        Err(error) if empty_system_roots(&error) => {
            let certificates = webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .map(|certificate| reqwest::Certificate::from_der(certificate.as_ref()))
                .collect::<Result<Vec<_>, _>>()?;
            builder().tls_certs_only(certificates).build()
        }
        Err(error) => Err(error),
    }
}

fn empty_system_roots(error: &reqwest::Error) -> bool {
    if !error.is_builder() {
        return false;
    }
    let mut source = error.source();
    while let Some(error) = source {
        // rustls-platform-verifier exposes this condition as General, rather
        // than a dedicated variant. Match its exact typed error, not arbitrary
        // builder failures or TLS verification errors from a request.
        if matches!(
            error.downcast_ref::<rustls::Error>(),
            Some(rustls::Error::General(message))
                if message == "No CA certificates were loaded from the system"
        ) {
            return true;
        }
        source = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn unrelated_builder_error_is_not_retried() {
        let attempts = AtomicUsize::new(0);
        let error = build_http_client(|| {
            attempts.fetch_add(1, Ordering::Relaxed);
            reqwest::Client::builder()
                .min_tls_version(reqwest::tls::Version::TLS_1_3)
                .max_tls_version(reqwest::tls::Version::TLS_1_2)
        })
        .unwrap_err();
        assert!(error.is_builder());
        assert!(!empty_system_roots(&error));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn empty_roots_retry_preserves_http_settings() {
        use std::io::Read;
        use std::net::TcpListener;
        use std::process::Command;
        use std::time::Duration;

        if let Ok(url) = std::env::var("ATTUNE_TLS_SETTINGS_TEST_URL") {
            let attempts = AtomicUsize::new(0);
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("x-tls-test", "preserved".parse().unwrap());
            let client = build_http_client(|| {
                attempts.fetch_add(1, Ordering::Relaxed);
                reqwest::Client::builder()
                    .timeout(Duration::from_millis(100))
                    .default_headers(headers.clone())
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
            })
            .unwrap();
            assert_eq!(attempts.load(Ordering::Relaxed), 2);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let error = runtime
                .block_on(async { client.get(url).send().await })
                .unwrap_err();
            assert!(error.is_timeout(), "fallback must retain the HTTP timeout");
            return;
        }

        let roots = tempfile::tempdir().unwrap();
        let bundle = roots.path().join("empty.pem");
        std::fs::write(&bundle, "").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("SSL_CERT_FILE", bundle)
            .env("SSL_CERT_DIR", roots.path())
            .env("ATTUNE_TLS_SETTINGS_TEST_URL", url)
            .args([
                "--exact",
                "http_client::tests::empty_roots_retry_preserves_http_settings",
                "--nocapture",
                "--test-threads=4",
            ])
            .output()
            .unwrap();
        roots.close().expect("remove owned empty trust store");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = String::new();
        socket.read_to_string(&mut request).unwrap();
        assert!(request
            .to_ascii_lowercase()
            .contains("x-tls-test: preserved"));
    }
}
