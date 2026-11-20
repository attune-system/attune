//! The five original cache assertion bodies, with actual separate DB principals.
use super::helpers;
use super::*;
use attune_common::repositories::rollout_admin::RolloutAdminRepository;
use serde_json::{json, Value};
use sqlx::Connection;
#[path = "../cache_cases/live.rs"]
mod live;
#[path = "../cache_cases/snapshot.rs"]
mod snapshot;

async fn operator_connection(f: &Fixture, name: &str, password: &str) -> sqlx::PgConnection {
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(name).unwrap();
    url.set_password(Some(password)).unwrap();
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-csearch_path={},public", f.db.schema()),
    );
    sqlx::PgConnection::connect(url.as_str()).await.unwrap()
}

async fn verify_operator(f: &Fixture, name: &str, password: &str, expected: bool) {
    let mut conn = operator_connection(f, name, password).await;
    let result =
        RolloutAdminRepository::verify(&mut conn, f.db.schema(), std::slice::from_ref(&f.roles[2]))
            .await;
    conn.close().await.unwrap();
    assert_eq!(
        result.is_ok(),
        expected,
        "unexpected verifier result: {result:?}"
    );
}

async fn rerun_and_rotate(f: &Fixture, mut inventory: Value, replacement: Value) {
    use super::admin_process::apply_provisioning;

    // The default contract still rejects the candidate owner membership.
    assert!(apply_provisioning(f, &inventory).await.is_err());
    inventory["cache_runtime_profile"] = json!(true);
    apply_provisioning(f, &inventory).await.unwrap();
    apply_provisioning(f, &inventory).await.unwrap();
    let mut wrong_inventory = inventory.clone();
    wrong_inventory["cache_owner"] = replacement.clone();
    assert!(apply_provisioning(f, &wrong_inventory).await.is_err());

    let old_operator = inventory["operator"].clone();
    let replacement_name = replacement["name"].as_str().unwrap();
    let password = format!("dummy_{}", Uuid::new_v4().simple());
    let ddl: String =
        sqlx::query_scalar("SELECT format('ALTER ROLE %I PASSWORD %L',$1::TEXT,$2::TEXT)")
            .bind(replacement_name)
            .bind(&password)
            .fetch_one(&*f.db)
            .await
            .unwrap();
    sqlx::query(&ddl).execute(&*f.db).await.unwrap();
    inventory["operation"] = json!("rotate");
    inventory["previous_operator"] = old_operator.clone();
    inventory["operator"] = replacement.clone();

    let old_connection = operator_connection(f, &f.roles[1], &f.passwords[0]).await;
    let busy_result = apply_provisioning(f, &inventory).await;
    old_connection.close().await.unwrap();
    assert!(busy_result.is_err());
    verify_operator(f, &f.roles[1], &f.passwords[0], true).await;
    verify_operator(f, replacement_name, &password, false).await;
    let (first, second) = tokio::join!(
        apply_provisioning(f, &inventory),
        apply_provisioning(f, &inventory)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    verify_operator(f, &f.roles[1], &f.passwords[0], false).await;
    verify_operator(f, replacement_name, &password, true).await;
    let retired_privileges: (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT pg_has_role($1,$2,'MEMBER'),
                has_schema_privilege($1,current_schema(),'USAGE'),
                has_table_privilege($1,'recorded_rollout_active','SELECT'),
                has_function_privilege($1,'issue_recorded_rollout_receipt(jsonb)','EXECUTE')",
    )
    .bind(&f.roles[1])
    .bind(&f.roles[0])
    .fetch_one(&*f.db)
    .await
    .unwrap();
    assert_eq!(retired_privileges, (false, false, false, false));

    inventory["previous_operator"] = replacement.clone();
    inventory["operator"] = old_operator;
    apply_provisioning(f, &inventory).await.unwrap();
    verify_operator(f, &f.roles[1], &f.passwords[0], true).await;
    verify_operator(f, replacement_name, &password, false).await;
}
#[tokio::test]
async fn representative_existing_regressions_with_actual_roles() {
    use futures::FutureExt;
    for upgrade in [false, true] {
        for case in 0..5 {
            let mut f = Fixture::new(false).await;
            let result=std::panic::AssertUnwindSafe(async {
                super::admin_process::provision(&mut f).await;
                let (inventory, replacement)=super::admin_process::provisioning_assets(&mut f).await;
                let context=super::cache_boundary::install(&mut f,upgrade).await;
                if case==0 {
                    super::cache_boundary::profile_denials(&f).await;
                    rerun_and_rotate(&f,inventory,replacement).await;
                    let mut url=url::Url::parse(f.db.database_url()).unwrap();
                    url.set_username(&f.roles[2]).unwrap(); url.set_password(Some(&f.passwords[1])).unwrap();
                    let config=attune_common::config::DatabaseConfig {url:url.to_string(),schema:Some(f.db.schema().into()),max_connections:1,min_connections:0,connect_timeout:10,idle_timeout:30,log_statements:false};
                    let runtime=attune_common::db::Database::new(&config).await.unwrap();
                    f.pools.push(runtime.pool().clone()); // Fixture joins this pool even if a later assertion panics.
                    assert_eq!(sqlx::query_scalar::<_,String>("SELECT session_user::TEXT").fetch_one(runtime.pool()).await.unwrap(),f.roles[2]);
                    runtime.close().await;
                }
                if let Some(g)=context.legacy {
                    // Legacy data/owner were verified BEFORE explicit maintenance reclamation;
                    // selected existing assertions require their original empty-data baseline.
                    super::cache_boundary::maintenance_drop(&mut f,g).await;
                }
                let maintenance=super::cache_boundary::maintenance_pool(&mut f).await;
                let api=f.pools[1].clone();
                let admin_name:String=sqlx::query_scalar("SELECT session_user::TEXT").fetch_one(&*f.db).await.unwrap();
                let (api_name,api_current,create):(String,String,bool)=sqlx::query_as("SELECT session_user::TEXT,current_user::TEXT,has_schema_privilege(current_user,current_schema(),'CREATE')").fetch_one(&api).await.unwrap();
                let (maint_name,maint_current):(String,String)=sqlx::query_as("SELECT session_user::TEXT,current_user::TEXT").fetch_one(&maintenance).await.unwrap();
                assert_ne!(admin_name,api_name);assert_ne!(maint_name,api_name);assert_ne!(admin_name,maint_name);assert_eq!(api_name,api_current);assert_eq!(maint_name,maint_current);assert!(!create);
                for ddl in ["CREATE TABLE denied(id BIGINT)","ALTER TABLE cache_entry ADD COLUMN denied BIGINT","DROP TABLE cache_entry"] {
                    let mut tx=api.begin().await.unwrap();assert!(sqlx::query(ddl).execute(&mut *tx).await.is_err());tx.rollback().await.unwrap();
                }
                match case {
                    0=>snapshot::promotion_is_optimistic_and_pinned_reads_do_not_mix_generations(&api,&maintenance).await,
                    1=>snapshot::refresh_reuse_race_preserves_one_producer_and_upload_contract(&api,&maintenance).await,
                    2=>snapshot::partition_cap_allows_retry_and_releases_capacity_only_after_drop(&api,&maintenance).await,
                    3=>live::live_replaces_and_deletes_in_one_durable_partition_with_exact_accounting(&api).await,
                    4=>live::live_initial_quota_failure_leaves_no_generation_partition_or_retry_receipt(&api).await,
                    _=>unreachable!(),
                }
                verify_operator(&f,&f.roles[1],&f.passwords[0],true).await;
                maintenance.close().await;
                eprintln!("existing cache assertion case{case}, populated_pre9={upgrade}: PASS with admin/API/maintenance logins");
            }).catch_unwind().await;
            f.cleanup().await;
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
    }
}
