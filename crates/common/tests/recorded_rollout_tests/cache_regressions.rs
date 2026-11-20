//! The five original cache assertion bodies, with actual separate DB principals.
use super::helpers;
use super::*;
use sqlx::Connection;
#[path = "../cache_cases/live.rs"]
mod live;
#[path = "../cache_cases/snapshot.rs"]
mod snapshot;

async fn verify(f: &Fixture) {
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(&f.roles[1]).unwrap();
    url.set_password(Some(&f.passwords[0])).unwrap();
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-csearch_path={},public", f.db.schema()),
    );
    let mut conn = sqlx::PgConnection::connect(url.as_str()).await.unwrap();
    attune_common::repositories::rollout_admin::RolloutAdminRepository::verify(
        &mut conn,
        f.db.schema(),
        std::slice::from_ref(&f.roles[2]),
    )
    .await
    .unwrap();
    conn.close().await.unwrap();
}
#[tokio::test]
async fn representative_existing_regressions_with_actual_roles() {
    use futures::FutureExt;
    for upgrade in [false, true] {
        for case in 0..5 {
            let mut f = Fixture::new(false).await;
            let result=std::panic::AssertUnwindSafe(async {
                super::admin_process::provision(&mut f).await;
                super::admin_process::provisioning_assets(&mut f).await;
                let context=super::cache_boundary::install(&mut f,upgrade).await;
                if case==0 {
                    super::cache_boundary::profile_denials(&f).await;
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
                verify(&f).await;
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
