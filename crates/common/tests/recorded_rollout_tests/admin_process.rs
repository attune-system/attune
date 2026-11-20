//! Owned process controls; all DB connections remain in the parent fixture.
use super::*;
use serde_json::{json, Value};
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;
const SECRET: &str = "dummy-rollout-db-password-not-for-output";
fn private(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
async fn output(args: &[&str]) -> std::process::Output {
    output_with_mask(args, false).await
}
async fn output_with_mask(args: &[&str], restrictive: bool) -> std::process::Output {
    use tokio::io::AsyncReadExt;
    let mut command = Command::new(env!("CARGO_BIN_EXE_attune-rollout-admin"));
    command
        .args(args)
        .env_clear()
        .env(
            "DATABASE_URL",
            format!("postgresql://bogus:{SECRET}@127.0.0.1:1/nope"),
        )
        .env("PGPASSWORD", SECRET)
        .env("ATTUNE_API_TOKEN", SECRET)
        .env("ENCRYPTION_KEY", SECRET)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if restrictive {
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o777);
                Ok(())
            });
        }
    }
    let mut child = command.spawn().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(40), child.wait()).await;
    if result.is_err() {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        panic!("owned offline process timed out and was joined");
    }
    let status = result.unwrap().unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .take(65536)
        .read_to_end(&mut stdout)
        .await
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .take(65536)
        .read_to_end(&mut stderr)
        .await
        .unwrap();
    if let Some(i) = args.iter().position(|a| *a == "--credentials") {
        if let Ok(bytes) = std::fs::read(args[i + 1]) {
            if let Ok(doc) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(uri) = doc["database_url"].as_str() {
                    if let Ok(url) = url::Url::parse(uri) {
                        if let Some(password) = url.password() {
                            assert!(
                                !args.iter().any(|a| a.contains(password) || a.contains(uri)),
                                "credential in process argv"
                            );
                            // Only the unrelated dummy marker is deliberately supplied in env;
                            // the real role password is exclusively in the private file.
                            assert!(!SECRET.contains(password));
                            for output in [&stdout, &stderr] {
                                let s = String::from_utf8_lossy(output);
                                assert!(
                                    !s.contains(password) && !s.contains(uri),
                                    "database credential disclosed"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    for bytes in [&stdout, &stderr] {
        let out = String::from_utf8_lossy(bytes);
        assert!(
            !out.contains(SECRET) && !out.contains("postgresql://") && !out.contains("nonce"),
            "offline process disclosed protected data"
        );
        if let Some(i) = args.iter().position(|a| *a == "--receipt") {
            if let Ok(bytes) = std::fs::read(args[i + 1]) {
                if let Ok(doc) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(nonce) = doc["nonce"].as_str() {
                        assert!(!out.contains(nonce), "receipt token disclosed");
                    }
                }
            }
        }
    }
    std::process::Output {
        status,
        stdout,
        stderr,
    }
}
async fn run(args: &[&str], ok: bool) -> Value {
    let output = output(args).await;
    assert_eq!(
        output.status.success(),
        ok,
        "unexpected offline process status"
    );
    serde_json::from_slice(if ok { &output.stdout } else { &output.stderr }).unwrap()
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}
async fn credential(f: &Fixture, dir: &Path, index: usize) -> std::path::PathBuf {
    let identity: Value = sqlx::query_scalar("SELECT recorded_rollout_installation()")
        .fetch_one(&*f.db)
        .await
        .unwrap();
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(&f.roles[index + 1]).unwrap();
    url.set_password(Some(&f.passwords[index])).unwrap();
    url.set_query(Some("sslmode=disable"));
    let path = dir.join(format!("credentials-{index}.json"));
    private(
        &path,
        &json!({"database_url":url.as_str(),"installation_schema":f.db.schema(),"installation_id":identity["uuid"],"database_oid":identity["database_oid"],"schema_oid":identity["schema_oid"],"capability_role":identity["capability_role"],"runtime_roles":[f.roles[2]]}),
    );
    path
}
pub(super) async fn provision(f: &mut Fixture) {
    let owner = format!("ws2_rollout_owner_{}", Uuid::new_v4().simple());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_roles WHERE rolname=$1")
            .bind(&owner)
            .fetch_one(&*f.db)
            .await
            .unwrap(),
        0
    );
    sqlx::query(&format!("CREATE ROLE {owner} NOLOGIN"))
        .execute(&*f.db)
        .await
        .unwrap();
    f.roles.push(owner.clone());
    std::fs::write(&f.manifest, serde_json::to_vec(&f.roles).unwrap()).unwrap();
    // Enumerate exact objects only in this test's owned application schema.
    let functions:Vec<String>=sqlx::query_scalar("SELECT format('ALTER FUNCTION %I.%I(%s) OWNER TO %I',n.nspname,p.proname,pg_get_function_identity_arguments(p.oid),$2) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.prokind='f'").bind(f.db.schema()).bind(&owner).fetch_all(&*f.db).await.unwrap();
    let relations:Vec<String>=sqlx::query_scalar("SELECT format('ALTER TABLE %I.%I OWNER TO %I',n.nspname,c.relname,$2) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p') ORDER BY c.oid").bind(f.db.schema()).bind(&owner).fetch_all(&*f.db).await.unwrap();
    for statement in relations.into_iter().chain(functions) {
        sqlx::query(&statement).execute(&*f.db).await.unwrap();
    }
    sqlx::query(&format!(
        "ALTER SCHEMA \"{}\" OWNER TO {owner}",
        f.db.schema()
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    // Revoke the deliberately broad test-only grants used by foundation negatives.
    for role in &f.roles[1..4] {
        sqlx::raw_sql(&format!("REVOKE ALL ON FUNCTION stage_recorded_rollout(JSONB),activate_recorded_rollout(UUID,TEXT,BIGINT,BIGINT) FROM {role}; REVOKE INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER ON recorded_rollout_generation FROM {role}")).execute(&*f.db).await.unwrap();
    }
    sqlx::raw_sql(&format!("GRANT SELECT ON recorded_rollout_installation_identity,recorded_rollout_offline_receipt,recorded_rollout_receipt_terminal TO {}; GRANT EXECUTE ON FUNCTION issue_recorded_rollout_receipt(JSONB),consume_recorded_rollout_receipt(JSONB),revoke_recorded_rollout_receipt(JSONB),recorded_rollout_admin_ledger() TO {}",f.roles[1],f.roles[1])).execute(&*f.db).await.unwrap();
}

pub(super) async fn cases(f: &mut Fixture) {
    use futures::FutureExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = std::panic::AssertUnwindSafe(cases_inner(f, root.path()))
        .catch_unwind()
        .await;
    root.close().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
async fn cases_inner(f: &mut Fixture, root: &Path) {
    let manifest = root.join("manifest.json");
    let expected = membership();
    private(&manifest, &serde_json::to_value(&expected).unwrap());
    let validated = run(&["validate", "--manifest", text(&manifest)], true).await;
    assert_eq!(
        validated["manifest"]["snapshot"],
        serde_json::to_value(&expected.snapshot).unwrap()
    );
    let creds = credential(f, root, 0).await;
    run(&["status", "--credentials", text(&creds)], false).await;
    run(
        &["verify-provisioning", "--credentials", text(&creds)],
        false,
    )
    .await; // shared owner is not production proof
    provision(f).await;
    provisioning_assets(f).await;
    let status = run(&["status", "--credentials", text(&creds)], true).await;
    assert_eq!(status["prior"], json!({"row_id":null,"revision":0}));
    run(
        &["verify-provisioning", "--credentials", text(&creds)],
        true,
    )
    .await;
    for index in [1, 2] {
        let denied = credential(f, root, index).await;
        run(
            &["verify-provisioning", "--credentials", text(&denied)],
            false,
        )
        .await;
    }
    // Missing grant and unexpected write privilege both deny without repair.
    sqlx::query(&format!("GRANT SELECT ON key TO {}", f.roles[1]))
        .execute(&*f.db)
        .await
        .unwrap();
    run(
        &["verify-provisioning", "--credentials", text(&creds)],
        false,
    )
    .await;
    sqlx::query(&format!("REVOKE SELECT ON key FROM {}", f.roles[1]))
        .execute(&*f.db)
        .await
        .unwrap();

    sqlx::query(&format!(
        "REVOKE SELECT ON recorded_rollout_offline_receipt FROM {}",
        f.roles[1]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    run(
        &["verify-provisioning", "--credentials", text(&creds)],
        false,
    )
    .await;
    sqlx::query(&format!(
        "GRANT SELECT,UPDATE ON recorded_rollout_offline_receipt TO {}",
        f.roles[1]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    run(
        &["verify-provisioning", "--credentials", text(&creds)],
        false,
    )
    .await;
    sqlx::query(&format!(
        "REVOKE UPDATE ON recorded_rollout_offline_receipt FROM {}",
        f.roles[1]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    sqlx::query(&format!(
        "GRANT CREATE ON SCHEMA \"{}\" TO {}",
        f.db.schema(),
        f.roles[2]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    run(
        &["verify-provisioning", "--credentials", text(&creds)],
        false,
    )
    .await;
    sqlx::query(&format!(
        "REVOKE CREATE ON SCHEMA \"{}\" FROM {}",
        f.db.schema(),
        f.roles[2]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    let restrictive = root.join("umask-receipt");
    let mask_result = output_with_mask(
        &[
            "stage",
            "--credentials",
            text(&creds),
            "--manifest",
            text(&manifest),
            "--receipt",
            text(&restrictive),
        ],
        true,
    )
    .await;
    assert!(mask_result.status.success());
    assert_eq!(
        std::fs::metadata(&restrictive)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    run(
        &[
            "revoke",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&restrictive),
            "--confirm-generation",
            &expected.snapshot.rollout_generation.to_string(),
        ],
        true,
    )
    .await;
    let mut missing_password: Value =
        serde_json::from_slice(&std::fs::read(&creds).unwrap()).unwrap();
    let mut no_password =
        url::Url::parse(missing_password["database_url"].as_str().unwrap()).unwrap();
    no_password.set_password(None).unwrap();
    missing_password["database_url"] = json!(no_password.as_str());
    let missing_file = root.join("missing-password");
    private(&missing_file, &missing_password);
    let missing = run(&["status", "--credentials", text(&missing_file)], false).await;
    assert_eq!(missing["error"], "invalid_credentials_file");
    let receipt = root.join("receipt.json");
    run(
        &[
            "stage",
            "--credentials",
            text(&creds),
            "--manifest",
            text(&manifest),
            "--receipt",
            text(&receipt),
        ],
        true,
    )
    .await;
    let stale = root.join("stale.json");
    run(
        &[
            "stage",
            "--credentials",
            text(&creds),
            "--manifest",
            text(&manifest),
            "--receipt",
            text(&stale),
        ],
        true,
    )
    .await;
    let original: Value = serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(original["installation"], status["installation"]);
    let generation = expected.snapshot.rollout_generation.to_string();
    cross_database(f, root, &manifest, &receipt, &generation).await;
    run(
        &[
            "stage",
            "--credentials",
            text(&creds),
            "--manifest",
            text(&manifest),
            "--receipt",
            text(&receipt),
        ],
        false,
    )
    .await;
    for (field, value) in [
        ("digest", json!("b".repeat(64))),
        ("generation", json!(Uuid::new_v4())),
        ("row_id", json!(999999)),
    ] {
        let mut edited = original.clone();
        edited["target"][field] = value;
        let p = root.join("edited.json");
        private(&p, &edited);
        run(
            &[
                "activate",
                "--credentials",
                text(&creds),
                "--receipt",
                text(&p),
                "--confirm-generation",
                &generation,
            ],
            false,
        )
        .await;
    }
    let mut edited = original.clone();
    edited["prior"]["revision"] = json!(999);
    let p = root.join("edited.json");
    private(&p, &edited);
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&p),
            "--confirm-generation",
            &generation,
        ],
        false,
    )
    .await;
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&receipt),
            "--confirm-generation",
            &Uuid::new_v4().to_string(),
        ],
        false,
    )
    .await;
    let link = root.join("receipt-link");
    symlink(&receipt, &link).unwrap();
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&link),
            "--confirm-generation",
            &generation,
        ],
        false,
    )
    .await;
    std::fs::hard_link(&receipt, root.join("hardlink")).unwrap();
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&receipt),
            "--confirm-generation",
            &generation,
        ],
        false,
    )
    .await;
    std::fs::remove_file(root.join("hardlink")).unwrap();
    let (a, b) = tokio::join!(
        run_unchecked(&creds, &receipt, &generation),
        run_unchecked(&creds, &receipt, &generation)
    );
    assert_ne!(a, b);
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&receipt),
            "--confirm-generation",
            &generation,
        ],
        false,
    )
    .await;
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&stale),
            "--confirm-generation",
            &generation,
        ],
        false,
    )
    .await;
    let mut replacement = membership();
    replacement.members[0].boot = Uuid::new_v4();
    let replacement_manifest = root.join("replacement.json");
    private(
        &replacement_manifest,
        &serde_json::to_value(&replacement).unwrap(),
    );
    let replacement_receipt = root.join("replacement-receipt.json");
    run(
        &[
            "stage",
            "--credentials",
            text(&creds),
            "--manifest",
            text(&replacement_manifest),
            "--receipt",
            text(&replacement_receipt),
        ],
        true,
    )
    .await;
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&replacement_receipt),
            "--confirm-generation",
            &replacement.snapshot.rollout_generation.to_string(),
        ],
        true,
    )
    .await;
    let next = root.join("next.json");
    run(
        &[
            "stage",
            "--credentials",
            text(&creds),
            "--manifest",
            text(&manifest),
            "--receipt",
            text(&next),
        ],
        true,
    )
    .await;
    run(
        &[
            "revoke",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&next),
            "--confirm-generation",
            &generation,
        ],
        true,
    )
    .await;
    run(
        &[
            "activate",
            "--credentials",
            text(&creds),
            "--receipt",
            text(&next),
            "--confirm-generation",
            &generation,
        ],
        false,
    )
    .await;
    let mode = std::fs::metadata(&creds).unwrap().permissions();
    std::fs::set_permissions(&creds, std::fs::Permissions::from_mode(0o644)).unwrap();
    run(&["status", "--credentials", text(&creds)], false).await;
    std::fs::set_permissions(&creds, mode).unwrap();
    let bad = root.join("bad-credentials");
    symlink(&creds, &bad).unwrap();
    run(&["status", "--credentials", text(&bad)], false).await;
    let mut wrong: Value = serde_json::from_slice(&std::fs::read(&creds).unwrap()).unwrap();
    wrong["installation_id"] = json!(Uuid::new_v4());
    let wrong_path = root.join("wrong-credentials");
    private(&wrong_path, &wrong);
    run(&["status", "--credentials", text(&wrong_path)], false).await;
    expiry_and_file_failure(f, root, &creds, &manifest, &generation).await;
    lost_ack(f, root, &creds, &manifest, false).await;
    lost_ack(f, root, &creds, &manifest, true).await;
}
async fn run_unchecked(creds: &Path, receipt: &Path, generation: &str) -> bool {
    output(&[
        "activate",
        "--credentials",
        text(creds),
        "--receipt",
        text(receipt),
        "--confirm-generation",
        generation,
    ])
    .await
    .status
    .success()
}

async fn drain_redacted(child: &mut tokio::process::Child, nonce: &str) -> Option<Value> {
    use tokio::io::AsyncReadExt;
    let mut out = Vec::new();
    let mut err = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        pipe.take(65536).read_to_end(&mut out).await.unwrap();
    }
    if let Some(pipe) = child.stderr.take() {
        pipe.take(65536).read_to_end(&mut err).await.unwrap();
    }
    for bytes in [&out, &err] {
        let s = String::from_utf8_lossy(bytes);
        assert!(
            !s.contains(SECRET)
                && !s.contains("postgresql://")
                && !s.contains("nonce")
                && !s.contains(nonce),
            "protected offline output"
        );
    }
    serde_json::from_slice(&err).ok()
}

async fn lost_ack(f: &Fixture, root: &Path, creds: &Path, manifest: &Path, crash: bool) {
    use super::commit_relay::{Class, Decision, OwnedCommitRelay};
    use futures::FutureExt;
    let file = root.join(if crash {
        "crash-receipt"
    } else {
        "ack-receipt"
    });
    run(
        &[
            "stage",
            "--credentials",
            text(creds),
            "--manifest",
            text(manifest),
            "--receipt",
            text(&file),
        ],
        true,
    )
    .await;
    let doc: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let generation = doc["target"]["generation"].as_str().unwrap();
    let mut credentials: Value = serde_json::from_slice(&std::fs::read(creds).unwrap()).unwrap();
    let mut url = url::Url::parse(credentials["database_url"].as_str().unwrap()).unwrap();
    let target = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap_or(5432));
    let mut relay = OwnedCommitRelay::start(target, 0, vec![Class::Publication])
        .await
        .unwrap();
    url.set_port(Some(relay.port)).unwrap();
    credentials["database_url"] = json!(url.as_str());
    let routed = root.join("routed-credentials");
    private(&routed, &credentials);
    let mut child = Command::new(env!("CARGO_BIN_EXE_attune-rollout-admin"))
        .args([
            "activate",
            "--credentials",
            text(&routed),
            "--receipt",
            text(&file),
            "--confirm-generation",
            generation,
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let result = std::panic::AssertUnwindSafe(async {
        let held = tokio::time::timeout(Duration::from_secs(15), relay.next_event())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.class, Class::Publication);
        assert_eq!(held.status, b'I');
        // Backend COMMIT is durable, but no acknowledgement reached the tool.
        if crash {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            // Client death may already have closed the receiver. The gate was
            // held until after SIGKILL/join, so no ACK reached that process.
            drop(held);
        } else {
            held.decide(Decision::Disconnect).unwrap();
        }
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !status.success(),
            "lost acknowledgement must not report success"
        );
    })
    .catch_unwind()
    .await;
    relay.release_holds();
    if child.try_wait().unwrap().is_none() {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
    let diagnostic = drain_redacted(&mut child, doc["nonce"].as_str().unwrap()).await;
    if !crash {
        assert_eq!(
            diagnostic.unwrap()["error"],
            "activation_acknowledgement_unknown"
        );
    }
    let report = relay.shutdown().await.unwrap();
    assert_eq!(report.publications, 1);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    let consumed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recorded_rollout_receipt_terminal t JOIN recorded_rollout_offline_receipt r ON r.id=t.receipt WHERE r.nonce=$1 AND t.outcome='consumed')").bind(doc["nonce"].as_str().unwrap().parse::<Uuid>().unwrap()).fetch_one(&*f.db).await.unwrap();
    assert!(consumed);
    run(
        &[
            "activate",
            "--credentials",
            text(creds),
            "--receipt",
            text(&file),
            "--confirm-generation",
            generation,
        ],
        false,
    )
    .await;
}

async fn expiry_and_file_failure(
    f: &Fixture,
    root: &Path,
    creds: &Path,
    manifest: &Path,
    generation: &str,
) {
    let expires = root.join("expiry-receipt");
    run(
        &[
            "stage",
            "--credentials",
            text(creds),
            "--manifest",
            text(manifest),
            "--receipt",
            text(&expires),
        ],
        true,
    )
    .await;
    let original: Value = serde_json::from_slice(&std::fs::read(&expires).unwrap()).unwrap();
    // Private DBA fixture ages immutable metadata to exercise the clock condition
    // without waiting fifteen minutes. Ordinary UPDATE remains trigger-denied.
    let nonce = original["nonce"].as_str().unwrap().parse::<Uuid>().unwrap();
    assert!(sqlx::query(
        "UPDATE recorded_rollout_offline_receipt SET expires=expires WHERE nonce=$1"
    )
    .bind(nonce)
    .execute(&*f.db)
    .await
    .is_err());
    let mut tx = f.db.begin().await.unwrap();
    sqlx::query("ALTER TABLE recorded_rollout_offline_receipt DISABLE TRIGGER recorded_offline_receipt_immutable").execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE recorded_rollout_offline_receipt SET issued=t.i,expires=t.e,body=jsonb_set(jsonb_set(body,'{issued}',to_jsonb(t.i)),'{expires}',to_jsonb(t.e)) FROM (SELECT clock_timestamp()-INTERVAL '20 minutes' i,clock_timestamp()-INTERVAL '10 minutes' e) t WHERE nonce=$1").bind(nonce).execute(&mut *tx).await.unwrap();
    sqlx::query("ALTER TABLE recorded_rollout_offline_receipt ENABLE TRIGGER recorded_offline_receipt_immutable").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let aged: Value =
        sqlx::query_scalar("SELECT body FROM recorded_rollout_offline_receipt WHERE nonce=$1")
            .bind(nonce)
            .fetch_one(&*f.db)
            .await
            .unwrap();
    private(&expires, &aged);
    let before = Repo::active(&mut f.db.acquire().await.unwrap())
        .await
        .unwrap();
    run(
        &[
            "activate",
            "--credentials",
            text(creds),
            "--receipt",
            text(&expires),
            "--confirm-generation",
            generation,
        ],
        false,
    )
    .await;
    assert_eq!(
        before,
        Repo::active(&mut f.db.acquire().await.unwrap())
            .await
            .unwrap()
    );
    let private_dir = root.join("output-failure");
    std::fs::create_dir(&private_dir).unwrap();
    std::fs::set_permissions(&private_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = private_dir.join("receipt");
    let previous_id: i64 =
        sqlx::query_scalar("SELECT COALESCE(max(id),0) FROM recorded_rollout_offline_receipt")
            .fetch_one(&*f.db)
            .await
            .unwrap();
    let mut lock = Some(f.db.begin().await.unwrap());
    sqlx::query("SELECT singleton FROM recorded_rollout_active FOR UPDATE")
        .execute(&mut **lock.as_mut().unwrap())
        .await
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_attune-rollout-admin"))
        .args([
            "stage",
            "--credentials",
            text(creds),
            "--manifest",
            text(manifest),
            "--receipt",
            text(&out),
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    use futures::FutureExt;
    let result = std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::fs::read_dir(&private_dir).unwrap().next().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        std::fs::set_permissions(&private_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        lock.take().unwrap().commit().await.unwrap();
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!status.success());
        assert!(!out.exists());
    })
    .catch_unwind()
    .await;
    if let Some(tx) = lock.take() {
        tx.rollback().await.unwrap();
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
    std::fs::set_permissions(&private_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    assert_eq!(
        before,
        Repo::active(&mut f.db.acquire().await.unwrap())
            .await
            .unwrap()
    );
    let id:i64=sqlx::query_scalar("SELECT id FROM recorded_rollout_offline_receipt WHERE NOT EXISTS(SELECT 1 FROM recorded_rollout_receipt_terminal t WHERE t.receipt=recorded_rollout_offline_receipt.id) ORDER BY id DESC LIMIT 1").fetch_one(&*f.db).await.unwrap();
    assert!(
        id > previous_id,
        "post-commit file failure did not issue its own receipt"
    );
    let nonce: String =
        sqlx::query_scalar("SELECT nonce::TEXT FROM recorded_rollout_offline_receipt WHERE id=$1")
            .bind(id)
            .fetch_one(&*f.db)
            .await
            .unwrap();
    let error = drain_redacted(&mut child, &nonce).await.unwrap();
    assert_eq!(error["error"], "receipt_output_failed");
    run(
        &[
            "revoke",
            "--credentials",
            text(creds),
            "--receipt-row",
            &id.to_string(),
            "--confirm-generation",
            generation,
        ],
        true,
    )
    .await;
}

async fn cross_database(
    f: &Fixture,
    root: &Path,
    manifest: &Path,
    receipt: &Path,
    generation: &str,
) {
    use futures::FutureExt;
    let mut second = Fixture::new(false).await;
    let result = std::panic::AssertUnwindSafe(async {
        let dir = root.join("other-installation");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        provision(&mut second).await;
        let creds = credential(&second, &dir, 0).await;
        run(
            &["verify-provisioning", "--credentials", text(&creds)],
            true,
        )
        .await;
        let second_receipt = dir.join("receipt");
        run(
            &[
                "stage",
                "--credentials",
                text(&creds),
                "--manifest",
                text(manifest),
                "--receipt",
                text(&second_receipt),
            ],
            true,
        )
        .await;
        // Same authored target/membership is not the same installed control plane.
        run(
            &[
                "activate",
                "--credentials",
                text(&creds),
                "--receipt",
                text(receipt),
                "--confirm-generation",
                generation,
            ],
            false,
        )
        .await;
        let primary: Value = sqlx::query_scalar("SELECT recorded_rollout_installation()")
            .fetch_one(&*f.db)
            .await
            .unwrap();
        let mut wrong: Value = serde_json::from_slice(&std::fs::read(&creds).unwrap()).unwrap();
        wrong["installation_id"] = primary["uuid"].clone();
        private(&creds, &wrong);
        run(&["status", "--credentials", text(&creds)], false).await;
    })
    .catch_unwind()
    .await;
    second.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

// Execute the shipped DBA SQL in the same transaction/temporary-input contract.
async fn apply_provisioning(f: &Fixture, inventory: &Value) -> sqlx::Result<()> {
    let mut tx = f.db.begin().await?;
    sqlx::query("CREATE TEMP TABLE rollout_provision_input(document JSONB) ON COMMIT DROP")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO rollout_provision_input VALUES ($1)")
        .bind(inventory)
        .execute(&mut *tx)
        .await?;
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deployment/rollout-admin/provision.sql"
    ))
    .unwrap();
    sqlx::raw_sql(&sql).execute(&mut *tx).await?;
    tx.commit().await
}
pub(super) async fn provisioning_assets(f: &mut Fixture) {
    let owner = f.roles[4].clone();
    let mut extra = Vec::new();
    for login in [true, false, true] {
        let role = format!("ws2_rollout_asset_{}", Uuid::new_v4().simple());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_roles WHERE rolname=$1")
                .bind(&role)
                .fetch_one(&*f.db)
                .await
                .unwrap(),
            0
        );
        sqlx::query(&format!(
            "CREATE ROLE {role} {}",
            if login { "LOGIN" } else { "NOLOGIN" }
        ))
        .execute(&*f.db)
        .await
        .unwrap();
        f.roles.push(role.clone());
        std::fs::write(&f.manifest, serde_json::to_vec(&f.roles).unwrap()).unwrap();
        extra.push(role);
    }
    sqlx::query(&format!(
        "GRANT {owner} TO {} WITH INHERIT FALSE, SET TRUE",
        extra[0]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for name in [
        &owner,
        &extra[0],
        &extra[1],
        &f.roles[1],
        &f.roles[0],
        &f.roles[2],
        &extra[2],
    ] {
        let oid: i64 = sqlx::query_scalar("SELECT oid::BIGINT FROM pg_roles WHERE rolname=$1")
            .bind(name)
            .fetch_one(&*f.db)
            .await
            .unwrap();
        ids.push(json!({"name":name,"oid":oid}));
    }
    let identity: Value = sqlx::query_scalar("SELECT recorded_rollout_installation()")
        .fetch_one(&*f.db)
        .await
        .unwrap();
    let mut inventory = json!({"version":1,"operation":"configure","schema":f.db.schema(),"installation":identity["uuid"],"owner":ids[0],"migrator":ids[1],"cache_owner":ids[2],"operator":ids[3],"capability":ids[4],"runtimes":[ids[5]],"previous_operator":null});
    // Existing broad inherited membership is refused, never silently repaired.
    assert!(apply_provisioning(f, &inventory).await.is_err());
    sqlx::raw_sql(&format!(
        "REVOKE {} FROM {}; GRANT {} TO {} WITH INHERIT FALSE, SET FALSE",
        f.roles[0], f.roles[1], f.roles[0], f.roles[1]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    apply_provisioning(f, &inventory).await.unwrap();
    apply_provisioning(f, &inventory).await.unwrap();
    let mut wrong = inventory.clone();
    wrong["operator"]["oid"] = json!(0);
    assert!(apply_provisioning(f, &wrong).await.is_err());
    sqlx::query(&format!(
        "GRANT CREATE ON SCHEMA \"{}\" TO {}",
        f.db.schema(),
        f.roles[2]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    assert!(apply_provisioning(f, &inventory).await.is_err());
    sqlx::query(&format!(
        "REVOKE CREATE ON SCHEMA \"{}\" FROM {}",
        f.db.schema(),
        f.roles[2]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    sqlx::query(&format!("GRANT {} TO {}", extra[1], f.roles[2]))
        .execute(&*f.db)
        .await
        .unwrap();
    assert!(apply_provisioning(f, &inventory).await.is_err());
    sqlx::query(&format!("REVOKE {} FROM {}", extra[1], f.roles[2]))
        .execute(&*f.db)
        .await
        .unwrap();
    // Old operator pools must be closed and joined before the transaction rotates.
    inventory["operation"] = json!("rotate");
    inventory["previous_operator"] = ids[3].clone();
    inventory["operator"] = ids[6].clone();
    assert!(apply_provisioning(f, &inventory).await.is_err());
    f.pools[0].close().await;
    let (first, second) = tokio::join!(
        apply_provisioning(f, &inventory),
        apply_provisioning(f, &inventory)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT pg_has_role($1,$2,'MEMBER')")
            .bind(&f.roles[1])
            .bind(&f.roles[0])
            .fetch_one(&*f.db)
            .await
            .unwrap()
    );
    // Restore via a deliberate reverse rotation for the subsequent one-shot suite.
    inventory["previous_operator"] = ids[6].clone();
    inventory["operator"] = ids[3].clone();
    apply_provisioning(f, &inventory).await.unwrap();
}
