//! Publication policy for repeated pack versions.

mod helpers;

use attune_common::{
    config::{PackRepublishConfig, PackRepublishMode, PacksConfig},
    models::Pack,
    repositories::{
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        FindById,
    },
};
use helpers::{create_test_pool, ActionFixture, PackFixture};
use serde_json::json;

fn release_input(pack: &Pack, version: &str, marker: char) -> CreatePackReleaseInput {
    let digest = marker.to_string().repeat(64);
    CreatePackReleaseInput {
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        version: version.to_string(),
        object_key: format!("packs/blobs/sha256/{digest}.tar.gz"),
        provider_version: format!("e:{digest}"),
        content_path: format!("/packs/.releases/sha256/{digest}"),
        digest,
        archive_size: 1,
        manifest: json!({"version": version, "marker": marker.to_string()}),
    }
}

fn republish_policy(mode: PackRepublishMode, suffix: &str) -> PacksConfig {
    PacksConfig {
        republish: PackRepublishConfig {
            mode,
            suffix: suffix.to_string(),
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn dev_version_republish_preserves_releases_and_reuses_any_identical_digest() {
    let database = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("dev-release")
        .with_version("1.2.3-dev")
        .create(&database)
        .await
        .unwrap();
    let enabled = republish_policy(PackRepublishMode::Suffix, "-dev");
    let mut tx = database.begin().await.unwrap();
    let first =
        PackReleaseRepository::create_or_get(&mut tx, release_input(&pack, "1.2.3-dev", 'a'))
            .await
            .unwrap();
    PackReleaseRepository::activate(&mut tx, pack.id, first.id)
        .await
        .unwrap();
    assert!(
        PackReleaseRepository::create_or_get(&mut tx, release_input(&pack, "1.2.3-dev", 'b'),)
            .await
            .is_err()
    );
    let second = PackReleaseRepository::create_or_get_with_policy(
        &mut tx,
        release_input(&pack, "1.2.3-dev", 'b'),
        &enabled,
    )
    .await
    .unwrap();
    assert_ne!(first.id, second.id);
    assert_ne!(first.digest, second.digest);
    PackReleaseRepository::activate(&mut tx, pack.id, second.id)
        .await
        .unwrap();
    assert!(
        PackReleaseRepository::create_or_get(&mut tx, release_input(&pack, "1.2.3-dev", 'c'),)
            .await
            .is_err()
    );
    for (marker, expected_id) in [('a', first.id), ('b', second.id)] {
        let repeated = PackReleaseRepository::create_or_get(
            &mut tx,
            release_input(&pack, "1.2.3-dev", marker),
        )
        .await
        .unwrap();
        assert_eq!(repeated.id, expected_id);
    }
    let latest = PackReleaseRepository::find_by_pack_version(&mut tx, pack.id, "1.2.3-dev")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, second.id);
    tx.commit().await.unwrap();

    let preserved = PackReleaseRepository::find_by_id(&database, first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preserved.digest, first.digest);
    assert_eq!(preserved.manifest, first.manifest);
    assert_eq!(preserved.content_path, first.content_path);
    assert!(preserved.inactive_since.is_some());
    assert_eq!(
        PackReleaseRepository::find_active_by_pack_ref(&database, &pack.r#ref)
            .await
            .unwrap()
            .unwrap()
            .id,
        second.id
    );
    assert_eq!(
        PackReleaseRepository::list_by_pack(&database, pack.id)
            .await
            .unwrap()
            .len(),
        2
    );
    database.cleanup().await.unwrap();
}

#[tokio::test]
async fn dev_version_republish_does_not_relax_other_version_bindings() {
    let database = create_test_pool().await.unwrap();
    let enabled = republish_policy(PackRepublishMode::Suffix, "-dev");
    for (index, version) in [
        "1.2.3",
        "1.2.3-rc",
        "1.2.3-dev.1",
        "1.2.3-DEV",
        "1.2.3-dev+build",
    ]
    .iter()
    .enumerate()
    {
        let pack = PackFixture::new_unique(&format!("strict-release-{index}"))
            .create(&database)
            .await
            .unwrap();
        let mut tx = database.begin().await.unwrap();
        PackReleaseRepository::create_or_get_with_policy(
            &mut tx,
            release_input(&pack, version, 'a'),
            &enabled,
        )
        .await
        .unwrap();
        let conflict = PackReleaseRepository::create_or_get_with_policy(
            &mut tx,
            release_input(&pack, version, 'b'),
            &enabled,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(conflict, attune_common::Error::AlreadyExists { .. }),
            "{version}: {conflict}"
        );
        tx.rollback().await.unwrap();
    }
    database.cleanup().await.unwrap();
}

#[tokio::test]
async fn dev_version_republish_keeps_existing_execution_pins() {
    let database = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("dev-pin")
        .with_version("1.2.3-dev")
        .create(&database)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "run")
        .create(&database)
        .await
        .unwrap();
    let enabled = republish_policy(PackRepublishMode::Suffix, "-dev");
    let mut tx = database.begin().await.unwrap();
    let first = PackReleaseRepository::create_or_get_with_policy(
        &mut tx,
        release_input(&pack, "1.2.3-dev", 'a'),
        &enabled,
    )
    .await
    .unwrap();
    let projections = PackProjectionIds {
        actions: vec![action.id],
        ..Default::default()
    };
    PackReleaseRepository::activate_projected(&mut tx, pack.id, first.id, &projections)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&database, action.id)
        .await
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &database,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let mut tx = database.begin().await.unwrap();
    let second = PackReleaseRepository::create_or_get_with_policy(
        &mut tx,
        release_input(&pack, "1.2.3-dev", 'b'),
        &enabled,
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(&mut tx, pack.id, second.id, &projections)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let pinned = ExecutionRepository::find_by_id(&database, execution.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pinned.pack_release, Some(first.id));
    assert_eq!(
        pinned.pack_release_digest.as_deref(),
        Some(first.digest.as_str())
    );
    assert_eq!(pinned.executable_snapshot.unwrap().release.id, first.id);
    let current = ExecutableSnapshotRepository::resolve_for_action(&database, action.id)
        .await
        .unwrap();
    assert_eq!(current.release.id, second.id);
    assert_eq!(current.release.digest, second.digest);
    database.cleanup().await.unwrap();
}

#[tokio::test]
async fn dev_version_republish_serializes_concurrent_identical_publications() {
    let database = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("concurrent-dev-release")
        .create(&database)
        .await
        .unwrap();
    let enabled = republish_policy(PackRepublishMode::Suffix, "-dev");
    let publish = || async {
        let mut tx = database.begin().await.unwrap();
        let release = PackReleaseRepository::create_or_get_with_policy(
            &mut tx,
            release_input(&pack, "1.2.3-dev", 'a'),
            &enabled,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        release
    };
    let (first, second) = tokio::join!(publish(), publish());
    assert_eq!(first.id, second.id);
    assert_eq!(
        PackReleaseRepository::list_by_pack(&database, pack.id)
            .await
            .unwrap()
            .len(),
        1
    );
    database.cleanup().await.unwrap();
}

#[tokio::test]
async fn pack_republish_off_serializes_conflicting_first_publications() {
    let database = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("concurrent-strict-release")
        .create(&database)
        .await
        .unwrap();
    let publish = |marker| {
        let database = &database;
        let pack = &pack;
        async move {
            let mut tx = database.begin().await.unwrap();
            let result =
                PackReleaseRepository::create_or_get(&mut tx, release_input(pack, "1.2.3", marker))
                    .await;
            if result.is_ok() {
                tx.commit().await.unwrap();
            } else {
                tx.rollback().await.unwrap();
            }
            result
        }
    };
    let (first, second) = tokio::join!(publish('a'), publish('b'));
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let conflict = first.err().or_else(|| second.err()).unwrap();
    assert!(matches!(
        conflict,
        attune_common::Error::AlreadyExists { .. }
    ));
    assert_eq!(
        PackReleaseRepository::list_by_pack(&database, pack.id)
            .await
            .unwrap()
            .len(),
        1,
    );
    database.cleanup().await.unwrap();
}

#[tokio::test]
async fn pack_republish_modes_gate_new_digests_and_preserve_idempotency() {
    let database = create_test_pool().await.unwrap();
    for (index, (mode, suffix, version, allowed)) in [
        (PackRepublishMode::Off, "-dev", "1.2.3-dev", false),
        (PackRepublishMode::On, "-dev", "1.2.3", true),
        (PackRepublishMode::On, "-dev", "1.2.3-rc", true),
        (PackRepublishMode::Suffix, "-dev", "1.2.3-alpha-dev", true),
        (
            PackRepublishMode::Suffix,
            "-snapshot",
            "1.2.3-snapshot",
            true,
        ),
        (PackRepublishMode::Suffix, "-snapshot", "1.2.3-dev", false),
        (PackRepublishMode::Suffix, "-snapshot", "1.2.3", false),
    ]
    .into_iter()
    .enumerate()
    {
        let pack = PackFixture::new_unique(&format!("republish-mode-{index}"))
            .with_version(version)
            .create(&database)
            .await
            .unwrap();
        let policy = republish_policy(mode, suffix);
        let mut tx = database.begin().await.unwrap();
        let first = PackReleaseRepository::create_or_get_with_policy(
            &mut tx,
            release_input(&pack, version, 'a'),
            &policy,
        )
        .await
        .unwrap();
        let result = PackReleaseRepository::create_or_get_with_policy(
            &mut tx,
            release_input(&pack, version, 'b'),
            &policy,
        )
        .await;
        if allowed {
            let second = result.unwrap();
            assert_ne!(first.id, second.id);
            assert_ne!(first.digest, second.digest);
            let rejected =
                PackReleaseRepository::create_or_get(&mut tx, release_input(&pack, version, 'c'))
                    .await
                    .unwrap_err();
            assert!(matches!(
                rejected,
                attune_common::Error::AlreadyExists { .. }
            ));
        } else {
            assert!(matches!(
                result.unwrap_err(),
                attune_common::Error::AlreadyExists { .. }
            ));
        }
        let repeated =
            PackReleaseRepository::create_or_get(&mut tx, release_input(&pack, version, 'a'))
                .await
                .unwrap();
        assert_eq!(repeated.id, first.id);
        tx.commit().await.unwrap();
    }
    database.cleanup().await.unwrap();
}
