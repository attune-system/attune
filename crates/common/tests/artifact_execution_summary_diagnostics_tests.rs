//! Execution-selection diagnostics must explain the real count-free reader.
mod helpers;

use attune_common::{
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactReadContext, ArtifactRepository, ArtifactSearchFilters,
            ArtifactVersionRepository, CreateArtifactInput, CreateArtifactVersionInput,
        },
        execution::{CreateExecutionInput, ExecutionRepository},
        Create,
    },
};
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn assert_plan_matches_page(plan: &Value, rows: usize) {
    // PostgreSQL 18 emits floating-point JSON here; PostgreSQL 16 emits integers.
    // Compare the exact small row count without rounding away fractional values.
    assert_eq!(plan[0]["Plan"]["Actual Rows"].as_f64(), Some(rows as f64));
    let output = plan[0]["Plan"]["Output"]
        .as_array()
        .expect("VERBOSE diagnostic must include the actual projection");
    for field in [".version", ".execution"] {
        assert!(output
            .iter()
            .any(|column| column.as_str().unwrap().contains(field)));
    }
    for column in output {
        let column = column.as_str().unwrap();
        for forbidden in [
            "content_json",
            "file_path",
            "object_key",
            "provider_version",
            ".meta",
            ".data",
        ] {
            assert!(
                !column.contains(forbidden),
                "unexpected projection: {column}"
            );
        }
    }
}

#[tokio::test]
async fn execution_summary_explain_matches_old_run_scoped_pages_and_cursor() -> Result<()> {
    let db = helpers::create_test_pool().await?;
    // These are repository read targets, not runtime authority/source fixtures.
    let old = ExecutionRepository::create(
        &db,
        CreateExecutionInput {
            action_ref: "diagnostic.run".into(),
            ..Default::default()
        },
    )
    .await?;
    let newer = ExecutionRepository::create(
        &db,
        CreateExecutionInput {
            action_ref: "diagnostic.run".into(),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = db.begin().await?;
    let mut references = Vec::new();
    for ordinal in 0..23 {
        let reference = format!("diagnostic.item{ordinal}");
        let artifact = ArtifactRepository::create(
            &mut *tx,
            CreateArtifactInput {
                r#ref: reference.clone(),
                scope: OwnerType::System,
                owner: String::new(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 100,
                name: None,
                description: Some("not a summary".into()),
                content_type: None,
                data: Some(json!({"private":true})),
            },
        )
        .await?;
        references.push(reference);
        for execution in [old.id, old.id, newer.id] {
            ArtifactVersionRepository::create(
                &mut *tx,
                CreateArtifactVersionInput {
                    artifact: artifact.id,
                    execution: Some(execution),
                    content_type: Some(
                        if execution == old.id {
                            "selected/type"
                        } else {
                            "newer/type"
                        }
                        .into(),
                    ),
                    content: None,
                    content_json: None,
                    file_path: None,
                    meta: Some(json!({"private":true})),
                    created_by: None,
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    // An unconstrained artifacts:read grant alone deliberately does not unlock
    // private linked artifacts. Keep that production visibility rule covered.
    let artifact_reader = ArtifactReadContext {
        identity_id: 1,
        identity_attributes: Default::default(),
        grants: serde_json::from_value(json!([{"resource":"artifacts","actions":["read"]}]))?,
    };
    let unrestricted = ArtifactReadContext {
        identity_id: 1,
        identity_attributes: Default::default(),
        // The linked executions really exist; their read grant makes all of
        // this fixture's private versions metadata-visible through that path.
        grants: serde_json::from_value(json!([{"resource":"executions","actions":["read"]}]))?,
    };
    // Exclude the newest artifact without changing the selected execution.
    let scoped = ArtifactReadContext {
        identity_id: 1,
        identity_attributes: Default::default(),
        grants: serde_json::from_value(json!([{"resource":"artifacts","actions":["read"],
            "constraints":{"refs": &references[..22]}}]))?,
    };
    let filters = ArtifactSearchFilters {
        execution: Some(old.id),
        limit: 21,
        ..Default::default()
    };
    assert!(ArtifactRepository::scan_execution_summaries(
        &db,
        &filters,
        Some(&artifact_reader),
        None,
    )
    .await?
    .is_empty());
    for (context, remaining) in [(&unrestricted, 3), (&scoped, 2)] {
        let first =
            ArtifactRepository::scan_execution_summaries(&db, &filters, Some(context), None)
                .await?;
        assert_eq!(first.len(), 21, "page of 20 plus one candidate");
        assert!(first
            .iter()
            .all(|row| row.selected_execution == Some(old.id)
                && row.selected_version == 2
                && row.selected_content_type.as_deref() == Some("selected/type")));
        let plan =
            ArtifactRepository::explain_execution_summary_scan(&db, &filters, Some(context), None)
                .await?;
        assert_plan_matches_page(&plan, 21);
        let after = Some((first[19].artifact.created, first[19].artifact.id));
        let next =
            ArtifactRepository::scan_execution_summaries(&db, &filters, Some(context), after)
                .await?;
        assert_eq!(next.len(), remaining);
        assert_eq!(next[0].artifact.id, first[20].artifact.id);
        assert!(next
            .iter()
            .all(|row| row.selected_execution == Some(old.id) && row.selected_version == 2));
        let plan =
            ArtifactRepository::explain_execution_summary_scan(&db, &filters, Some(context), after)
                .await?;
        assert_plan_matches_page(&plan, remaining);
    }
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_summary_diagnostic_requires_the_same_execution_selector() -> Result<()> {
    let db = helpers::create_test_pool().await?;
    let filters = ArtifactSearchFilters {
        limit: 21,
        ..Default::default()
    };
    assert!(
        ArtifactRepository::scan_execution_summaries(&db, &filters, None, None)
            .await
            .is_err()
    );
    assert!(
        ArtifactRepository::explain_execution_summary_scan(&db, &filters, None, None)
            .await
            .is_err()
    );
    db.cleanup().await?;
    Ok(())
}
