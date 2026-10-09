//! Trust-store tests run in fresh children so TLS environment changes never
//! affect another test or a cached platform verifier.

use attune_common::artifact_transport::{
    build_transport_with_worker_token_provider, ApiTransport, ArtifactFileTransport, TransportMode,
};
use attune_common::auth::{jwt::JwtConfig, WorkerTokenProvider};
use attune_common::pack_transport::build_pack_transport_with_worker_token_provider;
use base64::Engine;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Test-only CA and server key. The localhost leaf is valid from 2020 to 2120.
const CA: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDOTCCAiGgAwIBAgIUKgIj8Fl+LurEtC1x5WPxTYSxDlIwDQYJKoZIhvcNAQEL\n\
BQAwIzEhMB8GA1UEAwwYQXR0dW5lIFRMUyByZWdyZXNzaW9uIENBMCAXDTI2MTAw\n\
ODE4MDc1MFoYDzIxMjYwOTE0MTgwNzUwWjAjMSEwHwYDVQQDDBhBdHR1bmUgVExT\n\
IHJlZ3Jlc3Npb24gQ0EwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCv\n\
nTsMsFZIxQmTD5xKFkKCl4ZZ8YHS9KWF4IO92Fq90y6dm8amfxD6ov7cwTaBe3f3\n\
pp8g/PxpL6lk61+xHomKMAgz6Pu9FvE5eijallE4LmW5FFYvRtWDpQWN6IqLqhwL\n\
0ypj/EYPzQIZzi3hw9tYzegZxo1G4caVaTdxFgdqNjbYH220W98t4Jl1e9O8ETcC\n\
37ER5R/TUUylqq8K9TVGkwl+1FfbBFEJS214ilvbeIk0ffhn2+e//VT3PRj01xvl\n\
FEHLvh2xgyeSz4IBEkw3yV/zuJ4pi3//OJoIhL2Rko1S46iVpAksSlmuYDrEqa2U\n\
wsc0U0gMguld0PA7vRLxAgMBAAGjYzBhMB0GA1UdDgQWBBSckbHLWv+tjjLyNcpf\n\
2xlLWsHkIjAfBgNVHSMEGDAWgBSckbHLWv+tjjLyNcpf2xlLWsHkIjAPBgNVHRMB\n\
Af8EBTADAQH/MA4GA1UdDwEB/wQEAwIBBjANBgkqhkiG9w0BAQsFAAOCAQEAndu+\n\
/fBD9T7Hkljp3+NEVZrelk4KlQPNM+z7Jwh9KXMRdizU7cWbKn4SNhkgZNni6ITK\n\
dyPEzF9Nh8/JO5+u2RuOvUHVjB0xEOeKpQQgb9DDB94Yii1aw82Q8WQ7R/vwq8N+\n\
F4hDKcDxdDODaCKTCTYwEKm7leIey5oCRycppusW4e84RoKFrdoNWkjOJoEalqTs\n\
5Wlg1pBcMBR/p/cqO27gHZFdAjaB2eSxreOfkQh1RMfzKHcyZ8b1yau0/ijeNO3W\n\
KAHRfZToV2U7mzU46LHcPe2xCqUVnlNTIK0qYIFIu6ozr9Qb3hb82DwLoPRu2RbP\n\
IEggWYEOhTZ21Md/OA==\n\
-----END CERTIFICATE-----\n";
const SERVER_CERT: &str = "MIICeTCCAWGgAwIBAgIBAjANBgkqhkiG9w0BAQsFADAjMSEwHwYDVQQDDBhBdHR1bmUgVExTIHJlZ3Jlc3Npb24gQ0EwIBcNMjAwMTAxMDAwMDAwWhgPMjEyMDAxMDEwMDAwMDBaMBcxFTATBgNVBAMMDGxvY2FsaG9zdC1lYzBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABECt8sLXpyTnibnRa+Zwxi6Gpayl6BNytf3jvFBikPj/cvs/omCnlkfDUzvfXiWoYmKYhf3ypBfzo6B655AyduOjgYwwgYkwDAYDVR0TAQH/BAIwADAOBgNVHQ8BAf8EBAMCB4AwEwYDVR0lBAwwCgYIKwYBBQUHAwEwFAYDVR0RBA0wC4IJbG9jYWxob3N0MB0GA1UdDgQWBBRgROVMf8ulUC9ln9hga6MdT6ADazAfBgNVHSMEGDAWgBSckbHLWv+tjjLyNcpf2xlLWsHkIjANBgkqhkiG9w0BAQsFAAOCAQEAbHzmCiGf3gyMGGle5Vps6W8MuN0St2xSSid3MahDxuPSPafFJHXlWS1iBu7y6wNqTJj/dg+IUFFE6TaOSXWWH+1IjL/t4h9My01nW1RG/LSJ8FxlR28bOl+8htjanyMhBLBj6Akp5bxMjKzPZCxGOnAk7PnE9637FH1yA3Me+YkAZPvWu/93spreY/QR0Q7NwjDCjRDf5GwxvYOyVWvNP+6w5wdZzHRuote7QOsGGYUUqwh5ho/3BlO3kFnxT+fQLBLXLiI9cS0qzFkXTxELL2iWwXsti+nBUKoMiL6vqfhjMzp4S99xrFXeSW783gBYrXs8lHwamRgFd6TT+StFjg==";
const SERVER_KEY: &str = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgcCOtJBze8ggzdwFIWDY+64G4ZYMoK6dG+yTDrUJMM62hRANCAARArfLC16ck54m50WvmcMYuhqWspegTcrX947xQYpD4/3L7P6Jgp5ZHw1M7314lqGJimIX98qQX86OgeueQMnbj";

#[test]
fn worker_volume_transport_without_system_roots() {
    if std::env::var_os("ATTUNE_TLS_TEST_CHILD").is_some() {
        let provider = Arc::new(WorkerTokenProvider::new(
            1,
            "unregistered",
            JwtConfig {
                secret: "test-only-not-a-production-secret".into(),
                access_token_expiration: 60,
                refresh_token_expiration: 60,
            },
        ));
        // This is WorkerService's startup call, including the volume completion API.
        let transport = build_transport_with_worker_token_provider(
            "/artifacts",
            Some("http://127.0.0.1:9"),
            Some(provider),
            &TransportMode::Volume,
        )
        .expect("worker volume transport must initialize without system CA files");
        assert_eq!(transport.transport_mode(), "volume");
        return;
    }

    let roots = tempfile::tempdir().unwrap();
    let empty_bundle = roots.path().join("empty.pem");
    std::fs::write(&empty_bundle, "").unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("ATTUNE_TLS_TEST_CHILD", "1")
        .env("SSL_CERT_FILE", &empty_bundle)
        .env("SSL_CERT_DIR", roots.path())
        .args([
            "--exact",
            "worker_volume_transport_without_system_roots",
            "--nocapture",
            "--test-threads=4",
        ])
        .output()
        .unwrap();
    roots.close().expect("remove owned empty trust store");
    assert!(
        output.status.success(),
        "worker startup failed with an empty system trust store:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn api_transports_without_system_roots() {
    if std::env::var_os("ATTUNE_TLS_TEST_CHILD").is_some() {
        let provider = Arc::new(WorkerTokenProvider::new(
            1,
            "unregistered",
            JwtConfig {
                secret: "test-only-not-a-production-secret".into(),
                access_token_expiration: 60,
                refresh_token_expiration: 60,
            },
        ));
        for mode in [TransportMode::Api, TransportMode::Auto] {
            build_transport_with_worker_token_provider(
                "/artifacts",
                Some("http://127.0.0.1:9"),
                Some(provider.clone()),
                &mode,
            )
            .unwrap();
            build_pack_transport_with_worker_token_provider(
                "/packs",
                Some("http://127.0.0.1:9"),
                Some(provider.clone()),
                &mode,
            )
            .unwrap();
        }
        ApiTransport::new("http://127.0.0.1:9", "test-token", "/artifacts").unwrap();
        attune_common::pack_transport::ApiPackTransport::new(
            "http://127.0.0.1:9",
            "test-token",
            "/packs",
        )
        .unwrap();
        return;
    }
    run_child("api_transports_without_system_roots", "", None);
}

#[test]
fn system_private_ca_is_preserved() {
    tls_case("system_private_ca_is_preserved", CA, "localhost", true);
}

#[test]
fn bundled_fallback_rejects_untrusted_certificate() {
    tls_case(
        "bundled_fallback_rejects_untrusted_certificate",
        "",
        "localhost",
        false,
    );
}

#[test]
fn system_trust_still_verifies_hostname() {
    tls_case(
        "system_trust_still_verifies_hostname",
        CA,
        "127.0.0.1",
        false,
    );
}

fn run_child(test: &str, roots: &str, url: Option<&str>) {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("ca.pem");
    std::fs::write(&bundle, roots).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .env("ATTUNE_TLS_TEST_CHILD", "1")
        .env("SSL_CERT_FILE", &bundle)
        .env("SSL_CERT_DIR", root.path())
        .args(["--exact", test, "--nocapture", "--test-threads=4"]);
    if let Some(url) = url {
        command.env("ATTUNE_TLS_TEST_URL", url);
    }
    let output = command.output().unwrap();
    root.close().expect("remove owned test trust store");
    assert!(
        output.status.success(),
        "child {test} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn tls_case(test: &str, roots: &str, hostname: &str, trusted: bool) {
    if let Ok(url) = std::env::var("ATTUNE_TLS_TEST_URL") {
        let transport = ApiTransport::new(&url, "test-token", "/artifacts").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(transport.file_exists("test.txt"));
        if trusted {
            assert!(result.unwrap(), "private system CA should be trusted");
        } else {
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("API file_exists request failed"),
                "expected HTTPS request failure, got {error}"
            );
        }
        return;
    }

    let der = base64::engine::general_purpose::STANDARD;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![rustls::pki_types::CertificateDer::from(
            der.decode(SERVER_CERT).unwrap(),
        )],
        rustls::pki_types::PrivatePkcs8KeyDer::from(der.decode(SERVER_KEY).unwrap()).into(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = stop.clone();
    let server = std::thread::spawn(move || {
        while !server_stop.load(Ordering::Acquire) {
            let mut socket = match listener.accept() {
                Ok((socket, _)) => socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::park_timeout(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("TLS listener failed: {error}"),
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            let mut stream = rustls::Stream::new(&mut connection, &mut socket);
            let mut buffer = [0; 2048];
            if let Err(error) = stream.read(&mut buffer) {
                return Err(error.to_string());
            } else {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
                stream.flush().unwrap();
                return Ok(());
            }
        }
        Err("server stopped before receiving a TLS connection".into())
    });
    // Always stop and join the server, including when a child assertion fails.
    let result = std::panic::catch_unwind(|| {
        run_child(test, roots, Some(&format!("https://{hostname}:{port}")))
    });
    stop.store(true, Ordering::Release);
    server.thread().unpark();
    let handshake = server.join().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    if trusted {
        handshake.expect("private-CA TLS handshake should succeed");
    } else {
        let error = handshake.unwrap_err();
        assert!(
            error.contains("received fatal alert"),
            "expected TLS verification alert, got {error}"
        );
        println!("client rejected TLS certificate: {error}");
    }
}
