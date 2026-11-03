//! Count-free browse selection contracts at the real repository boundary.
mod helpers;

use attune_common::{
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactSearchFilters, ArtifactVersionRepository,
            CreateArtifactInput, CreateArtifactVersionInput,
        },
        Create,
    },
};

fn artifact_input(reference: &str) -> CreateArtifactInput {
    CreateArtifactInput {
        r#ref: reference.into(),
        scope: OwnerType::System,
        owner: String::new(),
        r#type: ArtifactType::FileText,
        visibility: ArtifactVisibility::Private,
        classification: ArtifactClassification::RuntimeLog,
        retention_policy: RetentionPolicyType::Versions,
        retention_limit: 1000,
        name: None,
        description: Some("not a metadata projection".into()),
        content_type: Some("parent/type".into()),
        data: Some(serde_json::json!({"secret": true})),
    }
}

#[tokio::test]
async fn created_id_cursor_handles_ties_and_does_not_return_newer_insertions() {
    let db = helpers::create_test_pool().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut ids = Vec::new();
    for name in ["cursor.first", "cursor.second", "cursor.third"] {
        ids.push(
            ArtifactRepository::create(&mut *tx, artifact_input(name))
                .await
                .unwrap()
                .id,
        );
    }
    let filters = ArtifactSearchFilters {
        limit: 2,
        ..Default::default()
    };
    let first = ArtifactRepository::scan_summaries(&mut *tx, &filters, None, None)
        .await
        .unwrap();
    assert_eq!(
        first.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1]]
    );
    assert_eq!(first[0].created, first[1].created);
    assert!(first[0].content_type.is_none());
    assert!(first[0].size_bytes.is_none());
    ArtifactRepository::create(&mut *tx, artifact_input("cursor.inserted"))
        .await
        .unwrap();
    let after = first.last().map(|row| (row.created, row.id));
    let second = ArtifactRepository::scan_summaries(&mut *tx, &filters, None, after)
        .await
        .unwrap();
    assert_eq!(
        second.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![ids[0]]
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn exact_execution_selection_and_history_cursor_exclude_bodies_and_locators() {
    let db = helpers::create_test_pool().await.unwrap();
    let artifact = ArtifactRepository::create(&db, artifact_input("cursor.selected"))
        .await
        .unwrap();
    let metadata = ArtifactRepository::find_metadata_by_id(&db, artifact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.retention_limit, 1000);
    assert_eq!(metadata.retention_policy, RetentionPolicyType::Versions);
    assert_eq!(
        ArtifactRepository::find_metadata_by_ref(&db, &artifact.r#ref)
            .await
            .unwrap()
            .unwrap()
            .id,
        artifact.id
    );
    let execution = 9_000_000_001_i64; // Deliberately dangling; retention never couples these IDs.
    let mut versions = Vec::new();
    for (run, content_type, bytes) in [
        (execution, "first/type", 2),
        (execution, "selected/type", 3),
        (execution + 1, "unrelated/type", 99),
    ] {
        versions.push(
            ArtifactVersionRepository::create(
                &db,
                CreateArtifactVersionInput {
                    artifact: artifact.id,
                    execution: Some(run),
                    content_type: Some(content_type.into()),
                    content: Some(vec![b'x'; bytes]),
                    content_json: None,
                    file_path: Some(format!("cursor/{bytes}.txt")),
                    meta: Some(serde_json::json!({"private": "never selected"})),
                    created_by: Some("arbitrary private creator".into()),
                },
            )
            .await
            .unwrap(),
        );
    }
    let filters = ArtifactSearchFilters {
        execution: Some(execution),
        limit: 21,
        ..Default::default()
    };
    let rows = ArtifactRepository::scan_execution_summaries(&db, &filters, None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].selected_id, versions[1].id);
    assert_eq!(rows[0].selected_version, 2);
    assert_eq!(rows[0].selected_execution, Some(execution));
    assert_eq!(
        rows[0].selected_content_type.as_deref(),
        Some("selected/type")
    );
    assert_eq!(rows[0].selected_size_bytes, Some(3));
    let page = ArtifactVersionRepository::scan_summaries(&db, artifact.id, None, 2, None)
        .await
        .unwrap();
    assert_eq!(
        page.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![versions[2].id, versions[1].id]
    );
    assert!(page.iter().all(|row| row.file_path.is_none()
        && row.object_key.is_none()
        && row.created_by.is_none()
        && row.provider_version.is_none()));
    let after = page.last().map(|row| (row.version, row.id));
    let next = ArtifactVersionRepository::scan_summaries(&db, artifact.id, None, 2, after)
        .await
        .unwrap();
    assert_eq!(
        next.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![versions[0].id]
    );
    db.cleanup().await.unwrap();
}
