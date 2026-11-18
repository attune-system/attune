//! Client initialization needs no cluster and must work before another TLS
//! consumer has selected the process provider.
#[test]
fn kubernetes_tls_client_initializes_in_fresh_process() {
    const CHILD: &str = "ATTUNE_KUBE_TLS_CLIENT_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut config = kube::Config::new("https://127.0.0.1:1".parse().unwrap());
                // No connection or host trust-store dependency. Empty trust
                // denies every server certificate; verification stays enabled.
                config.root_cert = Some(Vec::new());
                assert!(!config.accept_invalid_certs);
                let _client = kube::Client::try_from(config)
                    .expect("Kubernetes TLS client must select the workspace crypto provider");
            });
        return;
    }

    // Other tests may already have initialized Rustls. Use a fresh owned child
    // so that such initialization cannot hide the startup regression.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "kubernetes_tls_client_initializes_in_fresh_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fresh Kubernetes TLS client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn docker_client_initializes_in_fresh_process_without_kubernetes_preinitialization() {
    const CHILD: &str = "ATTUNE_DOCKER_CLIENT_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                // Construction only: this fixed nonexistent socket is never
                // connected. Kubernetes must not initialize Rustls for us.
                let _client = attune_workload_backend::engine::DockerEngine::connect(
                    std::path::Path::new("/tmp/attune-owned-client-test-never-connect.sock"),
                )
                .expect("Docker client must initialize independently of Kubernetes");
            });
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "docker_client_initializes_in_fresh_process_without_kubernetes_preinitialization",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fresh Docker client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
