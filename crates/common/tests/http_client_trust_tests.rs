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

#[path = "../../../tests/fixtures/tls.rs"]
mod tls;
use tls::{CA, SERVER_CERT, SERVER_KEY};

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
