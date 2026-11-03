use super::*;
use attune_common::child_process_environment::ChildProcessEnvironment;
use futures::FutureExt;
use std::collections::HashMap;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[path = "../../../../tests/fixtures/tls.rs"]
mod tls;

const CHILD: &str = "ATTUNE_SOURCE_AUTH_CHILD";
const ROOT: &str = "ATTUNE_SOURCE_AUTH_ROOT";
const TOKENS: [(&str, &str); 3] = [
    ("index", "Bearer fixture-index-credential"),
    ("git", "Basic fixture-git-credential"),
    ("archive", "Bearer fixture-archive-credential"),
];

#[tokio::test]
async fn managed_source_auth_git_install() -> Result<()> {
    run_case(false, "source_auth::managed_source_auth_git_install").await
}

#[tokio::test]
async fn managed_source_auth_git_failure_uses_authenticated_archive() -> Result<()> {
    run_case(
        true,
        "source_auth::managed_source_auth_git_failure_uses_authenticated_archive",
    )
    .await
}

fn assert_no_secrets(text: &str) {
    for (_, token) in TOKENS {
        assert!(
            !text.contains(token),
            "fixture credential escaped the source request"
        );
    }
}

fn git(directory: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    ChildProcessEnvironment::default().apply(&mut command);
    let output = command
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=Source fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .output()?;
    assert!(
        output.status.success(),
        "fixture Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn prepare(root: &Path, fail_git: bool) -> Result<()> {
    let pack_ref = if fail_git {
        "managed_fallback_source"
    } else {
        "managed_git_source"
    };
    let source = root.join("source");
    fs::create_dir(&source)?;
    fs::create_dir(root.join("public"))?;
    let manifest = format!("ref: {pack_ref}\nname: Private source fixture\nversion: 1.0.0\nauthor: Test\nlicense: MIT\ndescription: Private source integration test\ndependencies: []\n");
    fs::write(source.join("pack.yaml"), &manifest)?;
    git(&source, &["init", "--initial-branch=main"])?;
    git(&source, &["add", "pack.yaml"])?;
    git(&source, &["commit", "-m", "Private source fixture"])?;
    let revision = git(&source, &["rev-parse", "HEAD"])?;
    git(
        root,
        &[
            "clone",
            "--bare",
            source.to_str().unwrap(),
            root.join("public/pack.git").to_str().unwrap(),
        ],
    )?;
    git(&root.join("public/pack.git"), &["update-server-info"])?;
    let checksum = format!("sha256:{}", calculate_directory_checksum(&source)?);
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive.append_data(
        &mut header,
        format!("{pack_ref}/pack.yaml"),
        Cursor::new(manifest.as_bytes()),
    )?;
    let archive = archive.into_inner()?.finish()?;
    let archive_checksum = format!("sha256:{}", hex::encode(Sha256::digest(&archive)));
    fs::write(root.join("public/pack.tar.gz"), archive)?;
    fs::write(
        root.join("server.json"),
        serde_json::to_vec(&json!({
            "pack_ref": pack_ref, "git_ref": revision, "git_checksum": checksum,
            "archive_checksum": archive_checksum, "fail_git": fail_git,
            "tokens": TOKENS.into_iter().collect::<HashMap<_, _>>(),
        }))?,
    )?;
    fs::write(root.join("ca.pem"), tls::CA)?;
    fs::write(
        root.join("server.pem"),
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            tls::SERVER_CERT
        ),
    )?;
    fs::write(
        root.join("server.key"),
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            tls::SERVER_KEY
        ),
    )?;
    let real_git = std::env::split_paths(&std::env::var_os("PATH").ok_or("PATH is required")?)
        .map(|path| path.join("git"))
        .find(|path| path.is_file())
        .ok_or("git is required")?
        .canonicalize()?;
    fs::create_dir(root.join("bin"))?;
    let wrapper = root.join("bin/git");
    // Only supply this fixture's CA and record argv. Credentials still travel
    // through the real installer's environment into the real Git/libcurl process.
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nexec {} -c {} \"$@\"\n",
            shell_quote(&root.join("git-argv.txt")),
            shell_quote(&real_git),
            shell_quote(Path::new(&format!(
                "http.sslCAInfo={}",
                root.join("ca.pem").display()
            )))
        ),
    )?;
    fs::set_permissions(wrapper, fs::Permissions::from_mode(0o700))?;
    fs::create_dir(root.join("empty-ca-directory"))?;
    Ok(())
}

async fn run_case(fail_git: bool, test_name: &str) -> Result<()> {
    if std::env::var_os(CHILD).is_some() {
        return managed_install(
            &PathBuf::from(std::env::var_os(ROOT).ok_or("missing fixture root")?),
            fail_git,
        )
        .await;
    }
    let root = tempfile::tempdir()?;
    prepare(root.path(), fail_git)?;
    let mut server = tokio::process::Command::new("python3")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/pack_registry_tests/source_auth_server.py"),
        )
        .arg(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let outcome = AssertUnwindSafe(async {
        let mut ready = BufReader::new(server.stdout.take().ok_or("missing fixture stdout")?);
        let mut line = String::new();
        let bytes =
            tokio::time::timeout(Duration::from_secs(15), ready.read_line(&mut line)).await??;
        assert!(bytes > 0, "HTTPS fixture exited before readiness");
        let _: HashMap<String, String> = serde_json::from_str(&line)?;
        fs::write(root.path().join("origins.json"), line)?;
        let path = std::env::join_paths(std::iter::once(root.path().join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))?;
        // TLS trust and PATH changes are confined to a fresh test subprocess.
        // No verifier caches, process-global environments, or host trust stores are mutated.
        let mut child = tokio::process::Command::new(std::env::current_exe()?);
        child
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .env("PATH", path)
            .env("TMPDIR", root.path())
            .env("SSL_CERT_FILE", root.path().join("ca.pem"))
            .env("SSL_CERT_DIR", root.path().join("empty-ca-directory"))
            .args(["--exact", test_name, "--nocapture", "--test-threads=4"])
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(300), child.output()).await??;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_no_secrets(&stdout);
        assert_no_secrets(&stderr);
        assert!(
            output.status.success(),
            "managed source child failed:\n{stdout}\n{stderr}"
        );
        let argv = fs::read_to_string(root.path().join("git-argv.txt"))?;
        assert_no_secrets(&argv);
        for option in [
            "clone",
            "http.followRedirects=false",
            "http.proxy=",
            "http.sslVerify=true",
            "http.curloptResolve=",
        ] {
            assert!(
                argv.contains(option),
                "real installer Git invocation was not exercised"
            );
        }
        let records: Vec<serde_json::Value> =
            fs::read_to_string(root.path().join("requests.jsonl"))?
                .lines()
                .map(serde_json::from_str)
                .collect::<std::result::Result<_, _>>()?;
        assert!(records
            .iter()
            .all(|record| record["authorized"] == true && record["isolated"] == true));
        for role in ["index", "git"] {
            assert!(records.iter().any(|record| record["role"] == role));
        }
        assert_eq!(
            records.iter().any(|record| record["role"] == "archive"),
            fail_git
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .catch_unwind()
    .await;
    let stop_request = if let Some(mut stdin) = server.stdin.take() {
        stdin.write_all(b"stop\n").await
    } else {
        Ok(())
    };
    let stopped = tokio::time::timeout(Duration::from_secs(15), server.wait()).await;
    if stopped.is_err() {
        server.kill().await?;
        server.wait().await?;
    }
    let stderr = if let Some(mut pipe) = server.stderr.take() {
        use tokio::io::AsyncReadExt;
        let mut text = String::new();
        pipe.read_to_string(&mut text).await?;
        text
    } else {
        String::new()
    };
    assert_no_secrets(&stderr);
    root.close()?;
    match outcome {
        Ok(result) => {
            result?;
            stop_request?;
            assert!(
                stopped?.is_ok_and(|status| status.success()),
                "HTTPS fixture failed: {stderr}"
            );
            Ok(())
        }
        Err(panic) => {
            eprintln!("HTTPS fixture diagnostics: {stderr}");
            std::panic::resume_unwind(panic)
        }
    }
}

async fn managed_install(root: &Path, fail_git: bool) -> Result<()> {
    let origins: HashMap<String, String> =
        serde_json::from_slice(&fs::read(root.join("origins.json"))?)?;
    let config: serde_json::Value = serde_json::from_slice(&fs::read(root.join("server.json"))?)?;
    let ctx = TestContext::new()
        .await?
        .with_pack_registry(attune_common::config::PackRegistryConfig {
            indices: Vec::new(),
            approved_public_hosts: Vec::new(),
            approved_private_hosts: vec!["localhost".into()],
            allow_http: false,
            cache_enabled: false,
            verify_checksums: true,
            connect_timeout: 5,
            timeout: 15,
            ..Default::default()
        })
        .await?
        .with_admin_auth()
        .await?;
    let outcome = AssertUnwindSafe(async {
        // Disable the seeded public index so unselected resolution stays fully offline.
        for index in PackRegistryIndexRepository::list(&ctx.pool).await? {
            let response = ctx.put(&format!("/api/v1/pack-indices/{}", index.id), json!({"enabled": false}), None).await?;
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = ctx.post("/api/v1/pack-indices", json!({
            "url": format!("{}/index.json", origins["index"]),
            "headers": {"Authorization": TOKENS[0].1, "X-Pack-Source": "index", "X-Index-Only": TOKENS[0].1},
            "source_headers": {
                (origins["git"].clone()): {"Authorization": TOKENS[1].1, "X-Pack-Source": "git", "X-Git-Only": TOKENS[1].1},
                (origins["archive"].clone()): {"Authorization": TOKENS[2].1, "X-Pack-Source": "archive", "X-Archive-Only": TOKENS[2].1},
            }
        }), None).await?;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created: serde_json::Value = response.json().await?;
        assert_no_secrets(&created.to_string());
        let id = created["data"]["id"].as_i64().unwrap();
        let stored = PackRegistryIndexRepository::find_by_id(&ctx.pool, id).await?.unwrap();
        assert!(stored.headers.is_string());
        assert!(stored.source_headers.as_object().unwrap().values().all(serde_json::Value::is_string));
        assert_no_secrets(&stored.source_headers.to_string());
        // Cover both selected-managed and ordinary managed-index resolution.
        let mut request = json!({"source": config["pack_ref"], "skip_tests": true, "skip_deps": true});
        if fail_git { request["registry_id"] = id.into(); }
        let response = ctx.post("/api/v1/packs/install", request, None).await?;
        let status = response.status();
        let text = response.text().await?;
        assert_no_secrets(&text);
        assert_eq!(status, StatusCode::OK, "{text}");
        let body: serde_json::Value = serde_json::from_str(&text)?;
        assert_eq!(body["data"]["install_status"], "succeeded");
        let provenance = &body["data"]["provenance"];
        assert_eq!(provenance["registry_id"], id);
        assert_eq!(provenance["checksum_verified"], true);
        assert_eq!(provenance["fallback_occurred"], fail_git);
        assert_eq!(provenance["artifact_type"], if fail_git { "archive" } else { "git" });
        assert_eq!(provenance["checksum"], config[if fail_git { "archive_checksum" } else { "git_checksum" }]);
        let pack = PackRepository::find_by_ref(&ctx.pool, config["pack_ref"].as_str().unwrap()).await?.unwrap();
        assert!(Path::new(&ctx.state.config.packs_base_dir).join(&pack.r#ref).join("pack.yaml").is_file());
        ctx.flush_audit().await?;
        let details: Vec<Option<serde_json::Value>> = sqlx::query_scalar("SELECT details FROM audit_event")
            .fetch_all(&ctx.pool).await?;
        assert!(!details.is_empty());
        for detail in details.into_iter().flatten() { assert_no_secrets(&detail.to_string()); }
        Ok::<_, Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    ctx.cleanup().await?;
    match outcome {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
