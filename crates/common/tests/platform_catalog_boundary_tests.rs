//! Real bootstrap and concurrent-writer regressions, each in an owned schema.
mod helpers;

use attune_common::{
    config::Config,
    models::ManagementOrigin,
    platform_catalog::ManagedComponentKind,
    repositories::{
        action::{CreatePolicyInput, PolicyRepository},
        identity::{
            CreatePermissionAssignmentInput, PermissionAssignmentRepository,
            PermissionSetRepository,
        },
        pack::PackRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        pack_retention::PackRetentionRepository,
        platform_catalog::PlatformCatalogRepository as Catalog,
        runtime_version::{CreateRuntimeVersionInput, RuntimeVersionRepository},
        Create, FindById, FindByRef,
    },
};
use helpers::{create_test_pool, IdentityFixture, PackFixture, RuntimeFixture};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

async fn run_python_loader(db_url: &str, schema: &str, pack_root: &Path) -> std::process::Output {
    let python = std::env::var_os("ATTUNE_TEST_PYTHON").unwrap_or_else(|| "python3".into());
    let mut command = tokio::process::Command::new(python);
    command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/load_core_pack.py"))
        .args(["--schema", schema, "--pack-name", "core", "--pack-dir"])
        .arg(pack_root)
        .env("DATABASE_URL", db_url)
        .env("PGCONNECT_TIMEOUT", "5")
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(60), command.output()).await
        .expect("Python loader did not finish").expect("Python loader could not start; install psycopg2-binary and PyYAML or set ATTUNE_TEST_PYTHON")
}

async fn platform_snapshot(pool: &sqlx::PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object(
        'permissions', (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM permission_set p WHERE management_origin = 'platform'),
        'runtimes', (SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM runtime r WHERE management_origin = 'platform'),
        'versions', (SELECT jsonb_agg(to_jsonb(v) ORDER BY v.id) FROM runtime_version v JOIN runtime r ON r.id = v.runtime WHERE r.management_origin = 'platform'),
        'triggers', (SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM trigger t WHERE management_origin = 'platform'))")
        .fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "integration test - requires database and Python psycopg2-binary/PyYAML"]
async fn repeated_python_bootstrap_preserves_platform_and_rejects_changed_definitions() {
    let db = create_test_pool().await.unwrap();
    let config = Config::load_from_file(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config.test.yaml")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let pack = root.path().join("core");
    std::fs::create_dir(&pack).unwrap();
    std::fs::write(
        pack.join("pack.yaml"),
        "ref: core\nlabel: Core\nversion: 1.0.0\n",
    )
    .unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/core");
    for directory in ["permission_sets", "runtimes", "triggers"] {
        std::fs::create_dir(pack.join(directory)).unwrap();
        for entry in std::fs::read_dir(source.join(directory)).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), pack.join(directory).join(entry.file_name())).unwrap();
        }
    }
    // The current volume bootstrap runs before the first API reconciliation.
    let first = run_python_loader(&config.database.url, db.schema(), root.path()).await;
    assert!(
        first.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let core = PackRepository::get_by_ref(&db, "core").await.unwrap();
    let admin = PermissionSetRepository::get_by_ref(&db, "core.admin")
        .await
        .unwrap();
    let identity = IdentityFixture::new("bootstrap-admin")
        .create(&db)
        .await
        .unwrap();
    let assignment = PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: admin.id,
        },
    )
    .await
    .unwrap();
    Catalog::reconcile(&db).await.unwrap();
    let before = platform_snapshot(&db).await;
    for _ in 0..2 {
        let repeated = run_python_loader(&config.database.url, db.schema(), root.path()).await;
        assert!(
            repeated.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&repeated.stdout),
            String::from_utf8_lossy(&repeated.stderr)
        );
        assert_eq!(platform_snapshot(&db).await, before);
        assert_eq!(
            PermissionAssignmentRepository::get_by_id(&db, assignment.id)
                .await
                .unwrap()
                .permset,
            admin.id
        );
        assert_eq!(
            Catalog::origin(&db, ManagedComponentKind::PermissionSet, "core.key_creator")
                .await
                .unwrap(),
            Some(ManagementOrigin::Pack {
                pack_id: core.id,
                release_id: None
            })
        );
    }
    for (directory, filename) in [
        ("permission_sets", "admin.yaml"),
        ("runtimes", "python.yaml"),
        ("triggers", "alert.yaml"),
    ] {
        let path = pack.join(directory).join(filename);
        let original = std::fs::read_to_string(&path).unwrap();
        let mut changed: Value = serde_yaml_ng::from_str(&original).unwrap();
        changed["description"] = json!("changed by bootstrap");
        let pack_updated = PackRepository::get_by_id(&db, core.id)
            .await
            .unwrap()
            .updated;
        let permission_updated = PermissionSetRepository::get_by_ref(&db, "core.key_creator")
            .await
            .unwrap()
            .updated;
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let rejected = run_python_loader(&config.database.url, db.schema(), root.path()).await;
        assert!(!rejected.status.success(), "modified {filename} accepted");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("platform-owned"),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert_eq!(platform_snapshot(&db).await, before);
        assert_eq!(
            PackRepository::get_by_id(&db, core.id)
                .await
                .unwrap()
                .updated,
            pack_updated
        );
        assert_eq!(
            PermissionSetRepository::get_by_ref(&db, "core.key_creator")
                .await
                .unwrap()
                .updated,
            permission_updated
        );
        std::fs::write(&path, original).unwrap();
    }
    let path = pack.join("triggers/alert.yaml");
    let mut changed: Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    changed["enabled"] = json!(1);
    std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    let rejected = run_python_loader(&config.database.url, db.schema(), root.path()).await;
    assert!(
        !rejected.status.success(),
        "boolean-to-integer change is not an exact definition"
    );
    assert_eq!(platform_snapshot(&db).await, before);
    db.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn component_insert_waits_for_activation_and_does_not_pin_old_release() {
    let db = create_test_pool().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let pack = PackFixture::new("concurrent").create(&db).await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut releases = Vec::new();
    for (version, byte) in [("1.0.0", 'a'), ("2.0.0", 'b')] {
        let release = PackReleaseRepository::create_or_get(
            &mut tx,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                version: version.into(),
                digest: byte.to_string().repeat(64),
                object_key: format!("test/{version}"),
                provider_version: "test".into(),
                content_path: root.path().join(version).to_string_lossy().into_owned(),
                archive_size: 0,
                manifest: json!({}),
            },
        )
        .await
        .unwrap();
        releases.push(release);
    }
    PackReleaseRepository::activate(&mut tx, pack.id, releases[0].id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // T1 switches to R2 and completes the projection sweep, but does not commit.
    let mut activation = db.begin().await.unwrap();
    let activating_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *activation)
        .await
        .unwrap();
    PackReleaseRepository::activate(&mut activation, pack.id, releases[1].id)
        .await
        .unwrap();
    let mut connection = db.acquire().await.unwrap();
    let inserting_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    // T2 must wait for T1 rather than stamp the still-visible R1.
    let insert = tokio::spawn(async move {
        PolicyRepository::create(
            &mut *connection,
            CreatePolicyInput {
                r#ref: "concurrent.policy".into(),
                pack: Some(pack.id),
                pack_ref: Some(pack.r#ref),
                action: None,
                action_ref: None,
                enabled: true,
                priority: 0,
                parameters: vec![],
                method: None,
                threshold: None,
                rate_limit_max_executions: Some(5),
                rate_limit_window_seconds: Some(60),
                quotas: json!([]),
                name: "Concurrent insert".into(),
                description: None,
                tags: vec![],
            },
        )
        .await
    });
    let blocked = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let blockers: Vec<i32> = sqlx::query_scalar("SELECT pg_blocking_pids($1)")
                .bind(inserting_pid)
                .fetch_one(&mut *activation)
                .await
                .unwrap();
            if blockers.contains(&activating_pid) {
                break true;
            }
            if insert.is_finished() {
                break false;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("insert neither completed nor waited on activation");
    activation.commit().await.unwrap();
    let policy = tokio::time::timeout(Duration::from_secs(10), insert)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let origin = Catalog::origin_by_id(&db, ManagedComponentKind::Policy, policy.id)
        .await
        .unwrap();
    let retention = PackRetentionRepository::collect(
        &db,
        root.path(),
        chrono::Utc::now() + chrono::Duration::days(1),
        0,
        10,
    )
    .await;
    let old_release = PackReleaseRepository::find_by_id(&db, releases[0].id)
        .await
        .unwrap();
    db.cleanup().await.unwrap();
    assert!(
        blocked,
        "insert did not wait on activation; origin={origin:?}, retention={retention:?}"
    );
    assert_eq!(
        origin,
        Some(ManagementOrigin::Pack {
            pack_id: pack.id,
            release_id: Some(releases[1].id)
        })
    );
    assert_eq!(retention.unwrap().deleted, 1);
    assert!(old_release.is_none());
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn adoption_waits_for_version_writer_and_rejects_its_unexpected_child() {
    let db = create_test_pool().await.unwrap();
    let core = PackFixture::new("core").create(&db).await.unwrap();
    let runtime = RuntimeFixture::new(Some(core.id), Some("core".into()), "python")
        .create(&db)
        .await
        .unwrap();
    let mut writer = db.begin().await.unwrap();
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let child = RuntimeVersionRepository::create(
        &mut *writer,
        CreateRuntimeVersionInput {
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            version: "99".into(),
            version_major: Some(99),
            version_minor: None,
            version_patch: None,
            execution_config: json!({}),
            distributions: json!({}),
            is_default: false,
            available: true,
            meta: json!({}),
        },
    )
    .await
    .unwrap();
    let pool = db.pool().clone();
    let reconcile = tokio::spawn(async move { Catalog::reconcile(&pool).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation = 'runtime_version'::regclass
                 AND NOT granted AND mode = 'ShareRowExclusiveLock' AND $1 = ANY(pg_blocking_pids(pid)))"
            ).bind(writer_pid).fetch_one(&mut *writer).await.unwrap();
            if waiting { break; }
            assert!(!reconcile.is_finished(), "reconciliation did not wait for the version writer");
            tokio::task::yield_now().await;
        }
    }).await.expect("reconciler never reached the runtime-version lock");
    writer.commit().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(10), reconcile)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Unexpected runtime versions for core.python: 99"),
        "{error}"
    );
    assert_eq!(
        RuntimeVersionRepository::get_by_id(&db, child.id)
            .await
            .unwrap()
            .version,
        "99"
    );
    assert_eq!(
        Catalog::origin_by_id(&db, ManagedComponentKind::Runtime, runtime.id)
            .await
            .unwrap(),
        Some(ManagementOrigin::Pack {
            pack_id: core.id,
            release_id: None
        })
    );
    assert!(PermissionSetRepository::find_by_ref(&db, "core.admin")
        .await
        .unwrap()
        .is_none());
    db.cleanup().await.unwrap();
}
