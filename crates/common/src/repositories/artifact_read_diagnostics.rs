//! Explicit benchmark/operator diagnostics. Never called by artifact handlers.
use serde_json::Value;
use sqlx::PgPool;

use crate::Result;

pub struct ArtifactReadDiagnostics;

impl ArtifactReadDiagnostics {
    pub async fn wait_sample(pool: &PgPool) -> Result<(i64, i64)> {
        sqlx::query_as("SELECT count(*) FILTER (WHERE wait_event_type = 'Lock'), count(*) \
            FROM pg_stat_activity WHERE datid = (SELECT oid FROM pg_database WHERE datname = current_database())")
            .fetch_one(pool).await.map_err(Into::into)
    }

    pub async fn analyze_event_fixtures(pool: &PgPool) -> Result<()> {
        sqlx::query("ANALYZE event").execute(pool).await?;
        Ok(())
    }

    /// Per-relation bytes and catalog estimates, not exact bloat or a forecast.
    /// WAL position belongs to the whole owned server, not an individual request.
    pub async fn snapshot(pool: &PgPool) -> Result<Value> {
        let relations: Value = sqlx::query_scalar(
            "WITH RECURSIVE roots AS ( \
                SELECT to_regclass(name)::oid AS oid FROM unnest(ARRAY[ \
                    'artifact','artifact_version','execution','execution_secret_value','runtime_schema_binding', \
                    'event','event_schema_binding','execution_history','audit_event', \
                    'inquiry','inquiry_schema_binding','component_configuration_revision', \
                    'sensor_generation_schema_binding','sensor_rule_config_delivery']) name \
             ), relations AS ( \
                SELECT oid FROM roots WHERE oid IS NOT NULL \
                UNION SELECT i.inhrelid FROM pg_inherits i JOIN relations r ON i.inhparent = r.oid \
             ) SELECT COALESCE(jsonb_agg(jsonb_build_object( \
                'relation',c.relname,'kind',c.relkind,'estimated_rows',c.reltuples, \
                'table_bytes',pg_table_size(c.oid),'index_bytes',pg_indexes_size(c.oid), \
                'total_bytes',pg_total_relation_size(c.oid), \
                'toast_bytes',CASE WHEN c.reltoastrelid = 0 THEN 0 ELSE pg_total_relation_size(c.reltoastrelid) END, \
                'estimated_live_tuples',s.n_live_tup,'estimated_dead_tuples',s.n_dead_tup, \
                'last_analyze',s.last_analyze,'last_autoanalyze',s.last_autoanalyze, \
                'last_vacuum',s.last_vacuum,'last_autovacuum',s.last_autovacuum, \
                'analyze_count',s.analyze_count,'autoanalyze_count',s.autoanalyze_count \
             ) ORDER BY c.relname),'[]'::jsonb) FROM relations r JOIN pg_class c ON c.oid = r.oid \
             LEFT JOIN pg_stat_all_tables s ON s.relid = c.oid",
        ).fetch_one(pool).await?;
        let counts: Value = sqlx::query_scalar(
            "SELECT jsonb_build_object('artifact',(SELECT count(*) FROM artifact), \
             'artifact_version',(SELECT count(*) FROM artifact_version), \
             'execution',(SELECT count(*) FROM execution), \
             'event',(SELECT count(*) FROM event), \
             'audit_event',(SELECT count(*) FROM audit_event), \
             'expired_terminal_executions',(SELECT count(*) FROM execution \
                WHERE updated < NOW() - INTERVAL '1 hour' \
                AND status IN ('completed','failed','cancelled','timeout','abandoned')))",
        )
        .fetch_one(pool)
        .await?;
        let server: Value = sqlx::query_scalar(
            "SELECT jsonb_build_object('wal_position',pg_current_wal_lsn()::text, \
             'database_bytes',pg_database_size(current_database()), \
             'sessions',(SELECT count(*) FROM pg_stat_activity WHERE datid = (SELECT oid FROM pg_database WHERE datname = current_database())), \
             'lock_waiters',(SELECT count(*) FROM pg_stat_activity WHERE datid = (SELECT oid FROM pg_database WHERE datname = current_database()) AND wait_event_type = 'Lock'), \
             'pending_summary_invalidations',(SELECT count(*) FROM native_summary_invalidation))",
        ).fetch_one(pool).await?;
        Ok(
            serde_json::json!({"relations":relations,"exact_fixture_counts":counts,"owned_server":server,
            "pool_size":pool.size(),"pool_idle":pool.num_idle()}),
        )
    }
}
