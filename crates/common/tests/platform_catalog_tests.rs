//! These tests own their schemas. No installed core pack or live services are used.
mod helpers;

use attune_common::{
    config::{CacheAdmissionConfig, Config},
    models::ManagementOrigin,
    pack_registry::loader::PackComponentLoader,
    platform_catalog::{ManagedComponentKind as Kind, CATALOG_REVISION},
    repositories::{
        action::ActionRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository, UpdatePermissionSetInput,
        },
        pack::PackRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        platform_catalog::PlatformCatalogRepository as Catalog,
        runtime::{RuntimeRepository, UpdateRuntimeInput},
        runtime_version::{
            CreateRuntimeVersionInput, RuntimeVersionRepository, UpdateRuntimeVersionInput,
        },
        trigger::{SensorRepository, TriggerRepository, UpdateTriggerInput},
        Create, Delete, FindById, FindByRef, List, Update,
    },
    test_database::TestDatabase,
};
use helpers::{
    create_test_pool, ActionFixture, IdentityFixture, PackFixture, RuntimeFixture, SensorFixture,
    TriggerFixture,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::path::Path;

const MIGRATION: &str = "20260916000001_platform_catalog.sql";

async fn permission(pool: &PgPool, component_ref: &str, pack: Option<i64>) -> i64 {
    PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: component_ref.into(),
            pack,
            pack_ref: pack.map(|_| "core".into()),
            label: Some("Legacy".into()),
            description: None,
            grants: json!([]),
        },
    )
    .await
    .unwrap()
    .id
}

async fn snapshot(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'runtimes', (SELECT jsonb_agg(to_jsonb(r) ORDER BY r.ref) FROM runtime r),
          'versions', (SELECT jsonb_agg(to_jsonb(v) ORDER BY v.id) FROM runtime_version v),
          'permissions', (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.ref) FROM permission_set p),
          'triggers', (SELECT jsonb_agg(to_jsonb(t) ORDER BY t.ref) FROM trigger t),
          'handlers', (SELECT jsonb_agg(to_jsonb(h) ORDER BY h.ref) FROM intrinsic_handler h))",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn legacy_fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform_catalog");
    for directory in ["runtimes", "permission_sets", "triggers"] {
        std::fs::create_dir(root.path().join(directory)).unwrap();
        for entry in std::fs::read_dir(source.join(directory)).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(
                entry.path(),
                root.path().join(directory).join(entry.file_name()),
            )
            .unwrap();
        }
    }
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packs/core/permission_sets/key_creator.yaml"),
        root.path().join("permission_sets/key_creator.yaml"),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("actions")).unwrap();
    std::fs::write(
        root.path().join("actions/echo.yaml"),
        "ref: core.echo\nlabel: Echo\nrunner_type: shell\nentry_point: echo.sh\n",
    )
    .unwrap();
    std::fs::write(root.path().join("actions/echo.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    root
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn fresh_catalog_is_complete_idempotent_and_serialized_without_core() {
    let db = create_test_pool().await.unwrap();
    let (first, second) = tokio::join!(Catalog::reconcile(&db), Catalog::reconcile(&db));
    first.unwrap();
    second.unwrap();
    assert!(PackRepository::list(&db).await.unwrap().is_empty());
    assert_eq!(RuntimeRepository::list(&db).await.unwrap().len(), 9);
    assert_eq!(PermissionSetRepository::list(&db).await.unwrap().len(), 4);
    assert_eq!(TriggerRepository::list(&db).await.unwrap().len(), 3);
    assert_eq!(RuntimeVersionRepository::list(&db).await.unwrap().len(), 6);
    assert!(
        PermissionSetRepository::find_by_ref(&db, "core.key_creator")
            .await
            .unwrap()
            .is_none()
    );
    assert!(Catalog::intrinsic_handler(&db, "attune.inquiry/v1")
        .await
        .unwrap()
        .is_none());
    for (kind, component_ref) in [
        (Kind::Runtime, "core.native"),
        (Kind::PermissionSet, "core.admin"),
        (Kind::Trigger, "core.alert"),
    ] {
        assert_eq!(
            Catalog::origin(&db, kind, component_ref).await.unwrap(),
            Some(ManagementOrigin::Platform {
                catalog_revision: CATALOG_REVISION
            })
        );
    }
    let before = snapshot(&db).await;
    Catalog::reconcile(&db).await.unwrap();
    assert_eq!(snapshot(&db).await, before);
    db.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn maintenance_migration_preserves_builtin_ids_assignments_and_external_refs() {
    let config = Config::load_from_file(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config.test.yaml")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let db = TestDatabase::create_before(&config.database, MIGRATION)
        .await
        .unwrap()
        .with_cleanup_on_drop();
    let core = PackFixture::new("core").create(&db).await.unwrap();
    let other = PackFixture::new("external").create(&db).await.unwrap();
    let mut runtime_ids = Vec::new();
    for name in [
        "shell", "python", "nodejs", "native", "java", "ruby", "perl", "go", "r",
    ] {
        let runtime = RuntimeFixture::new(Some(core.id), Some("core".into()), name)
            .create(&db)
            .await
            .unwrap();
        runtime_ids.push((runtime.r#ref, runtime.id));
    }
    let mut permission_ids = Vec::new();
    let mut version_ids = Vec::new();
    for (runtime_ref, runtime_id) in &runtime_ids {
        let versions: &[&str] = match runtime_ref.as_str() {
            "core.python" => &["3.11", "3.12", "3.13"],
            "core.nodejs" => &["18", "20", "22"],
            _ => &[],
        };
        for version in versions {
            let (major, minor, patch) =
                attune_common::version_matching::extract_version_components(version);
            let row = RuntimeVersionRepository::create(
                &db,
                CreateRuntimeVersionInput {
                    runtime: *runtime_id,
                    runtime_ref: runtime_ref.clone(),
                    version: (*version).into(),
                    version_major: major,
                    version_minor: minor,
                    version_patch: patch,
                    execution_config: json!({}),
                    distributions: json!({}),
                    is_default: false,
                    available: false,
                    meta: json!({}),
                },
            )
            .await
            .unwrap();
            version_ids.push((row.id, row.runtime, row.version));
        }
    }
    for name in ["admin", "editor", "executor", "viewer"] {
        let component_ref = format!("core.{name}");
        permission_ids.push((
            component_ref.clone(),
            permission(&db, &component_ref, Some(core.id)).await,
        ));
    }
    let mut trigger_ids = Vec::new();
    for name in ["alert", "queue_started", "queue_empty"] {
        let trigger = TriggerFixture::new(Some(core.id), Some("core".into()), name)
            .create(&db)
            .await
            .unwrap();
        trigger_ids.push((trigger.r#ref, trigger.id));
    }
    let key_creator = permission(&db, "core.key_creator", Some(core.id)).await;
    let identity = IdentityFixture::new("catalog-admin")
        .create(&db)
        .await
        .unwrap();
    let assignment = PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_ids[0].1,
        },
    )
    .await
    .unwrap();
    let action = ActionFixture::new(other.id, "external", "uses_shell")
        .with_runtime(runtime_ids[0].1)
        .create(&db)
        .await
        .unwrap();
    // An operator relationship to a system trigger must also survive the transfer.
    sqlx::query("INSERT INTO rule (ref, pack, pack_ref, label, action, action_ref, trigger, trigger_ref, enabled) VALUES ('external.alert', $1, 'external', 'Alert', $2, 'external.uses_shell', $3, 'core.alert', true)")
        .bind(other.id).bind(action.id).bind(trigger_ids[0].1).execute(&db).await.unwrap();

    sqlx::raw_sql(include_str!(
        "../../../migrations/20260916000001_platform_catalog.sql"
    ))
    .execute(&db)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/20260916000002_catalog_bootstrap_and_release_lock.sql"
    ))
    .execute(&db)
    .await
    .unwrap();
    Catalog::reconcile(&db).await.unwrap();
    for (id, runtime_id, version) in &version_ids {
        let row = RuntimeVersionRepository::get_by_id(&db, *id).await.unwrap();
        assert_eq!(row.runtime, *runtime_id);
        assert_eq!(&row.version, version);
        assert!(
            !row.available,
            "host verification state must survive adoption"
        );
        assert_ne!(row.execution_config, json!({}));
    }
    for (component_ref, id) in &runtime_ids {
        assert_eq!(
            RuntimeRepository::get_by_ref(&db, component_ref)
                .await
                .unwrap()
                .id,
            *id
        );
    }
    for (component_ref, id) in &permission_ids {
        assert_eq!(
            PermissionSetRepository::get_by_ref(&db, component_ref)
                .await
                .unwrap()
                .id,
            *id
        );
    }
    for (component_ref, id) in &trigger_ids {
        assert_eq!(
            TriggerRepository::get_by_ref(&db, component_ref)
                .await
                .unwrap()
                .id,
            *id
        );
    }
    assert_eq!(
        PermissionAssignmentRepository::get_by_id(&db, assignment.id)
            .await
            .unwrap()
            .permset,
        permission_ids[0].1
    );
    assert_eq!(
        ActionRepository::get_by_id(&db, action.id)
            .await
            .unwrap()
            .runtime,
        Some(runtime_ids[0].1)
    );
    assert_eq!(
        Catalog::origin(&db, Kind::PermissionSet, "core.key_creator")
            .await
            .unwrap(),
        Some(ManagementOrigin::Pack {
            pack_id: core.id,
            release_id: None
        })
    );
    assert_eq!(
        PermissionSetRepository::get_by_ref(&db, "core.key_creator")
            .await
            .unwrap()
            .id,
        key_creator
    );
    let fixture = legacy_fixture();
    let config = CacheAdmissionConfig::default();
    let loader = PackComponentLoader::new(&db, core.id, "core", &config);
    let loaded = loader.load_all(fixture.path()).await.unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);

    // The first external-style release omits platform YAML but keeps core content.
    for directory in ["runtimes", "triggers"] {
        std::fs::remove_dir_all(fixture.path().join(directory)).unwrap();
    }
    for name in ["admin", "editor", "executor", "viewer"] {
        std::fs::remove_file(fixture.path().join(format!("permission_sets/{name}.yaml"))).unwrap();
    }
    let before = snapshot(&db).await;
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: core.id,
            pack_ref: "core".into(),
            version: "1.1.0".into(),
            digest: "a".repeat(64),
            object_key: "catalog-test/core.tar.gz".into(),
            provider_version: "test".into(),
            content_path: fixture.path().to_string_lossy().into_owned(),
            archive_size: 0,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    loader
        .load_all_in_transaction(&mut tx, fixture.path())
        .await
        .unwrap();
    PackReleaseRepository::activate(&mut tx, core.id, release.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // Pack-owned permissions can change, so compare the transferred rows directly.
    for (component_ref, id) in &permission_ids {
        assert_eq!(
            PermissionSetRepository::get_by_ref(&db, component_ref)
                .await
                .unwrap()
                .id,
            *id
        );
    }
    assert_eq!(snapshot(&db).await["runtimes"], before["runtimes"]);
    assert_eq!(
        Catalog::origin(&db, Kind::PermissionSet, "core.key_creator")
            .await
            .unwrap(),
        Some(ManagementOrigin::Pack {
            pack_id: core.id,
            release_id: Some(release.id)
        })
    );
    assert!(PackRepository::delete(&db, core.id).await.unwrap());
    assert_eq!(
        PermissionAssignmentRepository::get_by_id(&db, assignment.id)
            .await
            .unwrap()
            .permset,
        permission_ids[0].1
    );
    assert_eq!(
        ActionRepository::get_by_id(&db, action.id)
            .await
            .unwrap()
            .runtime,
        Some(runtime_ids[0].1)
    );
    let external_trigger: i64 =
        sqlx::query_scalar("SELECT trigger FROM rule WHERE ref = 'external.alert'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(external_trigger, trigger_ids[0].1);
    assert_eq!(RuntimeRepository::list(&db).await.unwrap().len(), 9);
    assert_eq!(PermissionSetRepository::list(&db).await.unwrap().len(), 4);
    assert_eq!(TriggerRepository::list(&db).await.unwrap().len(), 3);
    db.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn catalog_conflicts_abort_the_entire_reconciliation() {
    for conflict in [
        "ad_hoc",
        "wrong_pack",
        "auto_detected",
        "ad_hoc_trigger",
        "unexpected_python_version",
        "unexpected_shell_version",
    ] {
        let db = create_test_pool().await.unwrap();
        let core = PackFixture::new("core").create(&db).await.unwrap();
        match conflict {
            "ad_hoc" => {
                permission(&db, "core.admin", None).await;
            }
            "wrong_pack" => {
                let other = PackFixture::new("other").create(&db).await.unwrap();
                permission(&db, "core.admin", Some(other.id)).await;
            }
            "auto_detected" => {
                let runtime = RuntimeFixture::new(Some(core.id), Some("core".into()), "shell")
                    .create(&db)
                    .await
                    .unwrap();
                RuntimeRepository::update(
                    &db,
                    runtime.id,
                    UpdateRuntimeInput {
                        auto_detected: Some(true),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            }
            "ad_hoc_trigger" => {
                let trigger = TriggerFixture::new(Some(core.id), Some("core".into()), "alert")
                    .create(&db)
                    .await
                    .unwrap();
                sqlx::query("UPDATE trigger SET is_adhoc = true WHERE id = $1")
                    .bind(trigger.id)
                    .execute(&db)
                    .await
                    .unwrap();
            }
            "unexpected_python_version" | "unexpected_shell_version" => {
                let name = if conflict == "unexpected_python_version" {
                    "python"
                } else {
                    "shell"
                };
                let runtime = RuntimeFixture::new(Some(core.id), Some("core".into()), name)
                    .create(&db)
                    .await
                    .unwrap();
                RuntimeVersionRepository::create(
                    &db,
                    CreateRuntimeVersionInput {
                        runtime: runtime.id,
                        runtime_ref: runtime.r#ref,
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
            }
            _ => unreachable!(),
        }
        let before = snapshot(&db).await;
        assert!(Catalog::reconcile(&db).await.is_err(), "{conflict}");
        assert_eq!(snapshot(&db).await, before, "{conflict}");
        let revision: i32 = sqlx::query_scalar("SELECT revision FROM platform_catalog_state")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(revision, 0);
        db.cleanup().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn platform_mutations_fail_but_assignments_and_runtime_verification_work() {
    let db = create_test_pool().await.unwrap();
    Catalog::reconcile(&db).await.unwrap();
    let admin = PermissionSetRepository::get_by_ref(&db, "core.admin")
        .await
        .unwrap();
    assert!(PermissionSetRepository::update(
        &db,
        admin.id,
        UpdatePermissionSetInput {
            label: None,
            description: None,
            grants: Some(json!([]))
        }
    )
    .await
    .is_err());
    assert!(PermissionSetRepository::delete(&db, admin.id)
        .await
        .is_err());
    let runtime = RuntimeRepository::get_by_ref(&db, "core.shell")
        .await
        .unwrap();
    assert!(RuntimeRepository::update(
        &db,
        runtime.id,
        UpdateRuntimeInput {
            name: Some("replaced".into()),
            ..Default::default()
        }
    )
    .await
    .is_err());
    assert!(RuntimeRepository::delete(&db, runtime.id).await.is_err());
    let trigger = TriggerRepository::get_by_ref(&db, "core.alert")
        .await
        .unwrap();
    assert!(TriggerRepository::update(
        &db,
        trigger.id,
        UpdateTriggerInput {
            enabled: Some(false),
            ..Default::default()
        }
    )
    .await
    .is_err());
    assert!(TriggerRepository::delete(&db, trigger.id).await.is_err());
    let version = RuntimeVersionRepository::list(&db).await.unwrap().remove(0);
    assert!(RuntimeVersionRepository::delete(&db, version.id)
        .await
        .is_err());
    assert!(RuntimeVersionRepository::update(
        &db,
        version.id,
        UpdateRuntimeVersionInput {
            execution_config: Some(json!({})),
            ..Default::default()
        }
    )
    .await
    .is_err());
    RuntimeVersionRepository::update(
        &db,
        version.id,
        UpdateRuntimeVersionInput {
            available: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let identity = IdentityFixture::new("new-admin").create(&db).await.unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: admin.id,
        },
    )
    .await
    .unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn legacy_core_cannot_replace_platform_definitions_or_claim_ad_hoc_refs() {
    let db = create_test_pool().await.unwrap();
    Catalog::reconcile(&db).await.unwrap();
    let core = PackFixture::new("core").create(&db).await.unwrap();
    let fixture = legacy_fixture();
    let config = CacheAdmissionConfig::default();
    let loader = PackComponentLoader::new(&db, core.id, "core", &config);
    loader.load_all(fixture.path()).await.unwrap();
    let before = snapshot(&db).await;
    std::fs::write(
        fixture.path().join("permission_sets/admin.yaml"),
        "ref: core.admin\ngrants: []\n",
    )
    .unwrap();
    assert!(loader.load_all(fixture.path()).await.is_err());
    assert_eq!(snapshot(&db).await, before);
    std::fs::remove_file(fixture.path().join("permission_sets/admin.yaml")).unwrap();
    let trigger = TriggerFixture::new(Some(core.id), Some("core".into()), "operator")
        .create(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE trigger SET is_adhoc = true WHERE id = $1")
        .bind(trigger.id)
        .execute(&db)
        .await
        .unwrap();
    std::fs::write(
        fixture.path().join("triggers/operator.yaml"),
        "ref: core.operator\nlabel: Takeover\n",
    )
    .unwrap();
    assert!(loader.load_all(fixture.path()).await.is_err());
    std::fs::remove_file(fixture.path().join("triggers/operator.yaml")).unwrap();
    let runtime = RuntimeRepository::get_by_ref(&db, "core.shell")
        .await
        .unwrap();
    let sensor = SensorFixture::new(
        Some(core.id),
        Some("core".into()),
        runtime.id,
        runtime.r#ref,
        "operator_sensor",
    )
    .create(&db)
    .await
    .unwrap();
    sqlx::query("UPDATE sensor SET is_adhoc = true WHERE id = $1")
        .bind(sensor.id)
        .execute(&db)
        .await
        .unwrap();
    loader.load_all(fixture.path()).await.unwrap();
    assert!(SensorRepository::find_by_id(&db, sensor.id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        Catalog::origin_by_id(&db, Kind::Sensor, sensor.id)
            .await
            .unwrap(),
        Some(ManagementOrigin::AdHoc)
    );
    assert_eq!(
        TriggerRepository::get_by_id(&db, trigger.id)
            .await
            .unwrap()
            .label,
        "operator"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn newer_catalog_revision_and_incompatible_epoch_fail_closed() {
    for column in ["revision", "compatibility_epoch"] {
        let db = create_test_pool().await.unwrap();
        Catalog::reconcile(&db).await.unwrap();
        let before = snapshot(&db).await;
        sqlx::query(&format!("UPDATE platform_catalog_state SET {column} = 2"))
            .execute(&db)
            .await
            .unwrap();
        assert!(Catalog::reconcile(&db).await.is_err());
        assert!(Catalog::check_existing_epoch(&db).await.is_err());
        assert_eq!(snapshot(&db).await, before);
        db.cleanup().await.unwrap();
    }
}
