//! Integration tests for Dashboard repository behavior.

mod helpers;

use attune_common::{
    models::{DashboardScopeType, DashboardVisibility},
    repositories::{
        dashboard::{
            CreateDashboardInput, DashboardRepository, DashboardVersionRepository,
            UpdateDashboardInput,
        },
        Create, FindById,
    },
};
use helpers::create_test_pool;
use serde_json::{json, Value as JsonValue};

fn dashboard_spec(title: &str) -> JsonValue {
    json!({
        "layout": {
            "breakpoints": {
                "lg": { "min_width": 1280, "columns": 12 },
                "sm": { "min_width": 0, "columns": 4 }
            }
        },
        "data_sources": {
            "events": { "type": "event_count" }
        },
        "cards": [
            {
                "id": "events",
                "title": title,
                "source": "events",
                "position": {
                    "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                    "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                }
            }
        ]
    })
}

async fn create_dashboard(
    pool: &sqlx::PgPool,
    dashboard_ref: &str,
    label: &str,
    is_default_home: bool,
) -> attune_common::models::dashboard::Dashboard {
    DashboardRepository::create(
        pool,
        CreateDashboardInput {
            r#ref: dashboard_ref.to_string(),
            scope_type: DashboardScopeType::Global,
            scope_ref: "global".to_string(),
            pack: None,
            owner_identity: None,
            visibility: DashboardVisibility::Public,
            is_adhoc: false,
            label: label.to_string(),
            description: Some(format!("{} description", label)),
            enabled: true,
            is_default_home,
            spec_version: 1,
            spec: dashboard_spec(label),
            tags: vec!["test".to_string()],
            created_by: None,
        },
    )
    .await
    .expect("dashboard should be created")
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_dashboard_clears_existing_default_home_in_scope() {
    let pool = create_test_pool().await.expect("pool should be created");

    let first = create_dashboard(&pool, "core.home_one", "Home One", true).await;
    let second = create_dashboard(&pool, "core.home_two", "Home Two", true).await;

    let first = DashboardRepository::find_by_id(&pool, first.id)
        .await
        .expect("query should succeed")
        .expect("first dashboard should still exist");
    let second = DashboardRepository::find_by_id(&pool, second.id)
        .await
        .expect("query should succeed")
        .expect("second dashboard should exist");

    assert!(!first.is_default_home, "previous default should be cleared");
    assert!(second.is_default_home, "new dashboard should be default");

    let default_home = DashboardRepository::find_default_home_in_scope(
        &pool,
        DashboardScopeType::Global,
        "global",
    )
    .await
    .expect("default lookup should succeed")
    .expect("default dashboard should exist");
    assert_eq!(default_home.id, second.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_with_version_metadata_only_change_does_not_create_spec_revision() {
    let pool = create_test_pool().await.expect("pool should be created");

    let dashboard = create_dashboard(&pool, "core.ops_meta", "Ops", false).await;

    let updated = DashboardRepository::update_with_version(
        &pool,
        dashboard.id,
        UpdateDashboardInput {
            label: Some("Ops Updated".to_string()),
            expected_revision: Some(1),
            updated_by: Some(42),
            ..Default::default()
        },
    )
    .await
    .expect("metadata update should succeed");

    assert_eq!(updated.revision, 2);
    assert_eq!(updated.label, "Ops Updated");

    let versions = DashboardVersionRepository::list_by_dashboard(&pool, dashboard.id)
        .await
        .expect("version lookup should succeed");
    assert_eq!(
        versions.len(),
        1,
        "metadata-only updates should not add spec snapshots"
    );
    assert_eq!(versions[0].revision, 1);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_set_default_home_is_atomic_and_keeps_spec_history_clean() {
    let pool = create_test_pool().await.expect("pool should be created");

    let first = create_dashboard(&pool, "core.home_a", "Home A", true).await;
    let second = create_dashboard(&pool, "core.home_b", "Home B", false).await;

    let updated = DashboardRepository::set_default_home(&pool, second.id, Some(1), Some(7))
        .await
        .expect("set_default_home should succeed");

    let first = DashboardRepository::find_by_id(&pool, first.id)
        .await
        .expect("query should succeed")
        .expect("first dashboard should exist");
    let second = DashboardRepository::find_by_id(&pool, second.id)
        .await
        .expect("query should succeed")
        .expect("second dashboard should exist");

    assert_eq!(updated.id, second.id);
    assert_eq!(updated.revision, 2);
    assert!(!first.is_default_home);
    assert!(second.is_default_home);

    let versions = DashboardVersionRepository::list_by_dashboard(&pool, second.id)
        .await
        .expect("version lookup should succeed");
    assert_eq!(
        versions.len(),
        1,
        "default-home helper should not create spec revisions"
    );
    assert_eq!(versions[0].revision, 1);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_with_version_spec_change_creates_new_dashboard_version() {
    let pool = create_test_pool().await.expect("pool should be created");

    let dashboard = create_dashboard(&pool, "core.ops_spec", "Ops", false).await;
    let mut new_spec = dashboard.spec.clone();
    new_spec["cards"][0]["title"] = json!("Ops v2");

    let updated = DashboardRepository::update_with_version(
        &pool,
        dashboard.id,
        UpdateDashboardInput {
            spec: Some(new_spec),
            expected_revision: Some(1),
            updated_by: Some(99),
            ..Default::default()
        },
    )
    .await
    .expect("spec update should succeed");

    assert_eq!(updated.revision, 2);
    assert_eq!(updated.spec["cards"][0]["title"], "Ops v2");

    let versions = DashboardVersionRepository::list_by_dashboard(&pool, dashboard.id)
        .await
        .expect("version lookup should succeed");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].revision, 2);
    assert_eq!(versions[1].revision, 1);
    assert_eq!(versions[0].created_by, Some(99));
}
