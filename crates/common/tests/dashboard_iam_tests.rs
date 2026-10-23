mod helpers;

use attune_common::{
    rbac::{Action, Grant, Resource},
    repositories::{
        dashboard_iam::{DashboardIamFilters, DashboardIamRepository, DashboardIamScope},
        trigger::TriggerRepository,
        Delete,
    },
    runtime_authority::normalize_iam_grants,
};
use chrono::{Duration, Utc};
use helpers::{create_test_pool, EventFixture, PackFixture, TriggerFixture};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tracing::{
    field::{Field, Visit},
    instrument::WithSubscriber,
    Subscriber,
};
use tracing_subscriber::{layer::Context, prelude::*, Layer};

#[derive(Clone, Default)]
struct StatementBudget(Arc<AtomicUsize>);

impl<S: Subscriber> Layer<S> for StatementBudget {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        #[derive(Default)]
        struct Classify {
            driver: bool,
        }
        impl Visit for Classify {
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == "db.statement" {
                    let sql = value.to_ascii_lowercase();
                    self.driver = sql.contains("from pg_catalog.pg_type")
                        || sql.contains("from pg_catalog.pg_enum");
                }
            }
            fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
        }
        let mut classification = Classify::default();
        event.record(&mut classification);
        if !classification.driver {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}
use std::collections::HashMap;

#[tokio::test]
async fn local_dashboard_aggregation_filters_actual_owners_before_counts_and_rejects_ref_reuse() {
    let db = create_test_pool().await.unwrap();
    let local = PackFixture::new_unique("dashboard_local")
        .create(&db)
        .await
        .unwrap();
    let foreign = PackFixture::new_unique("dashboard_foreign")
        .create(&db)
        .await
        .unwrap();
    let trigger = TriggerFixture::new(Some(local.id), Some(local.r#ref.clone()), "event")
        .create(&db)
        .await
        .unwrap();
    let other = TriggerFixture::new(Some(foreign.id), Some(foreign.r#ref.clone()), "event")
        .create(&db)
        .await
        .unwrap();
    let retained = EventFixture::new(Some(trigger.id), &trigger.r#ref)
        .create(&db)
        .await
        .unwrap();
    EventFixture::new(Some(other.id), &other.r#ref)
        .create(&db)
        .await
        .unwrap();
    let grants: Vec<Grant> = serde_json::from_value(
        normalize_iam_grants(
            json!([{"resource":"events","actions":["read"]}]),
            Some(local.id),
        )
        .unwrap(),
    )
    .unwrap();
    let mut scope = DashboardIamScope {
        identity: 1,
        attributes: HashMap::new(),
        grants,
        resource: Resource::Events,
        action: Action::Read,
    };
    let now = Utc::now();
    let filters = DashboardIamFilters {
        source: "event_count",
        since: now - Duration::hours(2),
        until: now + Duration::hours(1),
        pack_refs: None,
        primary_refs: None,
        statuses: vec![],
        assigned_to: None,
        sla_target_seconds: 900,
        limit: 1,
        small_cohort_min: None,
    };
    let rows = DashboardIamRepository::query(&*db, &scope, &filters)
        .await
        .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["series"], trigger.r#ref);
    assert_eq!(rows[0]["count"], 1);
    scope.grants[0].constraints.as_mut().unwrap().ids = Some(vec![retained.id + 100000]);
    assert!(DashboardIamRepository::query(&*db, &scope, &filters)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    scope.grants[0].constraints.as_mut().unwrap().ids = None;
    let mut ids = vec![retained.id];
    for _ in 1..100 {
        ids.push(
            EventFixture::new(Some(trigger.id), &trigger.r#ref)
                .create(&db)
                .await
                .unwrap()
                .id,
        );
    }
    for count in [1, 64, 100] {
        scope.grants[0].constraints.as_mut().unwrap().ids = Some(ids[..count].to_vec());
        let budget = StatementBudget::default();
        let subscriber = tracing_subscriber::registry().with(budget.clone());
        let mut connection = db.acquire().await.unwrap();
        let rows = DashboardIamRepository::query(&mut *connection, &scope, &filters)
            .with_subscriber(subscriber)
            .await
            .unwrap();
        assert_eq!(rows[0]["count"], count as i64);
        assert_eq!(
            budget.0.load(Ordering::Relaxed),
            1,
            "one authorized source query for {count} targets"
        );
    }
    scope.grants[0].constraints.as_mut().unwrap().ids = None;
    TriggerRepository::delete(&*db, trigger.id).await.unwrap();
    let replacement = TriggerFixture::new(Some(local.id), Some(local.r#ref.clone()), "event")
        .create(&db)
        .await
        .unwrap();
    assert_ne!(trigger.id, replacement.id);
    assert!(DashboardIamRepository::query(&*db, &scope, &filters)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    EventFixture::new(Some(replacement.id), &replacement.r#ref)
        .create(&db)
        .await
        .unwrap();
    let rows = DashboardIamRepository::query(&*db, &scope, &filters)
        .await
        .unwrap();
    assert_eq!(rows[0]["count"], 1);
    db.cleanup().await.unwrap();
}
