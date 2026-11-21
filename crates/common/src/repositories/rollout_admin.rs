//! One-shot offline control repository. No runtime callers or readiness output.
use crate::{Error, Result};
use serde_json::{json, Value};
use sqlx::PgConnection;

fn denied() -> Error {
    Error::invalid_state("Offline rollout validation denied")
}
pub struct RolloutAdminRepository;
impl RolloutAdminRepository {
    pub async fn assert_identity(conn: &mut PgConnection, expected: &str) -> Result<()> {
        let matches: bool =
            sqlx::query_scalar("SELECT session_user::TEXT=$1 AND current_user=session_user")
                .bind(expected)
                .fetch_one(conn)
                .await?;
        if !matches {
            return Err(denied());
        }
        Ok(())
    }
    /// Catalog-only checks run BEFORE invoking installation-owned functions.
    pub async fn assert_trusted_schema(conn: &mut PgConnection, schema: &str) -> Result<()> {
        let trusted:bool=sqlx::query_scalar("SELECT pg_catalog.count(*)=2 AND pg_catalog.bool_and(NOT EXISTS(SELECT 1 FROM pg_catalog.aclexplode(n.nspacl) a WHERE a.privilege_type::pg_catalog.text='CREATE'::pg_catalog.text AND a.grantee<>n.nspowner) AND NOT pg_catalog.pg_has_role(session_user::pg_catalog.name,n.nspowner,'MEMBER'::pg_catalog.text) AND (n.nspname::pg_catalog.text<>$1::pg_catalog.text OR (NOT r.rolcanlogin AND NOT r.rolsuper AND NOT r.rolcreaterole AND NOT r.rolcreatedb AND NOT r.rolbypassrls AND NOT r.rolreplication))) FROM pg_catalog.pg_namespace n JOIN pg_catalog.pg_roles r ON r.oid=n.nspowner WHERE n.nspname::pg_catalog.text=ANY(ARRAY[$1::pg_catalog.text,'pg_catalog'::pg_catalog.text])").bind(schema).fetch_one(&mut *conn).await?;
        if !trusted {
            return Err(denied());
        }
        let names = [
            "recorded_rollout_operator_role",
            "recorded_rollout_operator",
            "recorded_rollout_installation",
            "stage_recorded_rollout",
            "activate_recorded_rollout",
            "issue_recorded_rollout_receipt",
            "consume_recorded_rollout_receipt",
            "revoke_recorded_rollout_receipt",
            "recorded_rollout_admin_ledger",
            "recorded_membership_valid",
            "canonical_recorded_membership",
            "recorded_membership_digest",
            "guard_recorded_rollout_write",
            "guard_recorded_offline_metadata",
        ];
        type FunctionTrustRow = (String, bool, Option<Vec<String>>);
        let rows:Vec<FunctionTrustRow>=sqlx::query_as("SELECT p.proname::pg_catalog.text,p.proowner=n.nspowner,p.proconfig FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname::pg_catalog.text=$1::pg_catalog.text AND p.proname::pg_catalog.text=ANY($2::pg_catalog.text[])").bind(schema).bind(names.to_vec()).fetch_all(&mut *conn).await?;
        let mut seen = std::collections::BTreeSet::new();
        let plain = format!("search_path=pg_catalog, {schema}, pg_temp");
        let quoted = format!(
            "search_path=pg_catalog, \"{}\", pg_temp",
            schema.replace('"', "\"\"")
        );
        for (name, owned, config) in rows {
            if !owned
                || !seen.insert(name)
                || !matches!(config,Some(values) if values==[plain.clone()] || values==[quoted.clone()])
            {
                return Err(denied());
            }
        }
        if seen.len() != names.len() {
            return Err(denied());
        }
        let tables = [
            "recorded_rollout_generation",
            "recorded_rollout_active",
            "recorded_rollout_installation_identity",
            "recorded_rollout_offline_receipt",
            "recorded_rollout_receipt_terminal",
            "_sqlx_migrations",
        ];
        let owned:bool=sqlx::query_scalar("SELECT pg_catalog.count(*)=6 AND pg_catalog.bool_and(c.relowner=n.nspowner AND c.relkind='r' AND NOT c.relrowsecurity AND NOT c.relforcerowsecurity) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname::pg_catalog.text=$1::pg_catalog.text AND c.relname::pg_catalog.text=ANY($2::pg_catalog.text[])").bind(schema).bind(tables.to_vec()).fetch_one(&mut *conn).await?;
        if !owned {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn status(conn: &mut PgConnection) -> Result<Value> {
        let (installation,prior):(Value,Value)=sqlx::query_as("SELECT recorded_rollout_installation(),jsonb_build_object('row_id',generation,'revision',revision) FROM recorded_rollout_active WHERE singleton").fetch_one(conn).await?;
        Ok(json!({"installation":installation,"prior":prior}))
    }
    pub async fn stage(conn: &mut PgConnection, manifest: &Value) -> Result<Value> {
        Ok(
            sqlx::query_scalar("SELECT issue_recorded_rollout_receipt($1)")
                .bind(manifest)
                .fetch_one(conn)
                .await?,
        )
    }
    pub async fn receipt(conn: &mut PgConnection, id: i64) -> Result<Value> {
        Ok(
            sqlx::query_scalar("SELECT body FROM recorded_rollout_offline_receipt WHERE id=$1")
                .bind(id)
                .fetch_one(conn)
                .await?,
        )
    }
    pub async fn activate(conn: &mut PgConnection, receipt: &Value) -> Result<()> {
        sqlx::query("SELECT consume_recorded_rollout_receipt($1)")
            .bind(receipt)
            .execute(conn)
            .await?;
        Ok(())
    }
    pub async fn revoke(conn: &mut PgConnection, receipt: &Value) -> Result<()> {
        sqlx::query("SELECT revoke_recorded_rollout_receipt($1)")
            .bind(receipt)
            .execute(conn)
            .await?;
        Ok(())
    }
    pub async fn verify(
        conn: &mut PgConnection,
        schema: &str,
        runtime_roles: &[String],
    ) -> Result<Value> {
        Self::assert_trusted_schema(conn, schema).await?;
        let (session,current,role,direct):(String,String,String,bool)=sqlx::query_as("SELECT session_user::TEXT,current_user::TEXT,recorded_rollout_operator_role()::TEXT,recorded_rollout_operator()").fetch_one(&mut *conn).await?;
        if session != current || !direct {
            return Err(denied());
        }
        let status = Self::status(conn).await?;
        if status["installation"]["schema"] != schema
            || status["installation"]["capability_role"] != role
        {
            return Err(denied());
        }
        let valid:bool=sqlx::query_scalar("SELECT NOT r.rolcanlogin AND NOT r.rolsuper AND NOT r.rolcreaterole AND NOT r.rolcreatedb AND NOT r.rolreplication AND NOT r.rolbypassrls AND NOT EXISTS(SELECT 1 FROM aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a WHERE a.privilege_type='CREATE' AND a.grantee<>n.nspowner) AND NOT has_schema_privilege(session_user,n.oid,'CREATE') AND NOT has_schema_privilege(r.oid,'pg_catalog','CREATE') AND NOT pg_has_role(session_user,n.nspowner,'MEMBER') FROM pg_namespace n JOIN pg_roles r ON r.oid=n.nspowner WHERE n.nspname=$1").bind(schema).fetch_one(&mut *conn).await?;
        if !valid {
            return Err(denied());
        }
        let catalog_safe:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM pg_namespace n CROSS JOIN LATERAL aclexplode(COALESCE(n.nspacl,acldefault('n',n.nspowner))) a WHERE n.nspname='pg_catalog' AND a.privilege_type='CREATE' AND a.grantee<>n.nspowner) AND NOT has_schema_privilege(session_user,'pg_catalog','CREATE')").fetch_one(&mut *conn).await?;
        if !catalog_safe {
            return Err(denied());
        }
        let login_ok:bool=sqlx::query_scalar("SELECT rolcanlogin AND NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolreplication AND NOT rolbypassrls AND NOT EXISTS(SELECT 1 FROM pg_auth_members m JOIN pg_roles r ON r.oid=m.roleid WHERE m.member=pg_roles.oid AND (r.rolname<>$1 OR m.admin_option)) FROM pg_roles WHERE rolname=session_user").bind(&role).fetch_one(&mut *conn).await?;
        if !login_ok {
            return Err(denied());
        }
        let cap_ok:bool=sqlx::query_scalar("SELECT NOT rolcanlogin AND NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolreplication AND NOT rolbypassrls AND NOT EXISTS(SELECT 1 FROM pg_auth_members m WHERE m.member=pg_roles.oid) FROM pg_roles WHERE rolname=$1").bind(&role).fetch_one(&mut *conn).await?;
        if !cap_ok {
            return Err(denied());
        }
        for name in [
            "recorded_rollout_operator_role",
            "recorded_rollout_operator",
            "recorded_membership_valid",
            "canonical_recorded_membership",
            "recorded_membership_digest",
            "guard_recorded_rollout_write",
            "recorded_rollout_installation",
            "guard_recorded_offline_metadata",
        ] {
            let safe:bool=sqlx::query_scalar("SELECT p.proowner=n.nspowner AND NOT p.prosecdef AND p.proconfig=ARRAY[format('search_path=pg_catalog, %I, pg_temp',n.nspname)] FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.proname=$2").bind(schema).bind(name).fetch_one(&mut *conn).await?;
            if !safe {
                return Err(denied());
            }
        }
        let tables = [
            "recorded_rollout_generation",
            "recorded_rollout_active",
            "recorded_rollout_installation_identity",
            "recorded_rollout_offline_receipt",
            "recorded_rollout_receipt_terminal",
        ];
        for table in tables {
            let valid:bool=sqlx::query_scalar("SELECT has_table_privilege(session_user,c.oid,'SELECT') AND NOT has_table_privilege(session_user,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') AND c.relowner=n.nspowner FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2").bind(schema).bind(table).fetch_one(&mut *conn).await?;
            if !valid {
                return Err(denied());
            }
        }
        let functions = [
            "issue_recorded_rollout_receipt",
            "consume_recorded_rollout_receipt",
            "revoke_recorded_rollout_receipt",
            "recorded_rollout_admin_ledger",
            "stage_recorded_rollout",
            "activate_recorded_rollout",
        ];
        for name in functions {
            let (safe,can):(bool,bool)=sqlx::query_as("SELECT p.prosecdef AND p.proowner=n.nspowner AND p.proconfig=ARRAY[format('search_path=pg_catalog, %I, pg_temp',n.nspname)] AND NOT EXISTS(SELECT 1 FROM aclexplode(COALESCE(p.proacl,acldefault('f',p.proowner))) a WHERE a.grantee NOT IN (p.proowner,(SELECT oid FROM pg_roles WHERE rolname=session_user)) AND a.privilege_type='EXECUTE'),has_function_privilege(session_user,p.oid,'EXECUTE') FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.proname=$2").bind(schema).bind(name).fetch_one(&mut *conn).await?;
            if !safe {
                return Err(denied());
            }
            // Old direct mutation functions must not bypass receipt consumption.
            if name == "stage_recorded_rollout" || name == "activate_recorded_rollout" {
                if can {
                    return Err(denied());
                }
            } else if !can {
                return Err(denied());
            }
        }
        // Exact offline scope: no writes (including column grants) anywhere in
        // the installation, and no reads of unrelated application tables.
        let extra_table_privilege:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p','v','m','f') AND (has_table_privilege(session_user,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') OR has_any_column_privilege(session_user,c.oid,'INSERT,UPDATE,REFERENCES') OR (c.relname::TEXT<>ALL($2::TEXT[]) AND has_any_column_privilege(session_user,c.oid,'SELECT')) OR EXISTS(SELECT 1 FROM aclexplode(COALESCE(c.relacl,acldefault('r',c.relowner))) a WHERE a.grantee IN (0,(session_user::regrole)::OID,(recorded_rollout_operator_role()::regrole)::OID) AND (a.is_grantable OR a.privilege_type<>'SELECT' OR c.relname::TEXT<>ALL($2::TEXT[])))))").bind(schema).bind(tables.to_vec()).fetch_one(&mut *conn).await?;
        let extra_function_grant:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace CROSS JOIN LATERAL aclexplode(COALESCE(p.proacl,acldefault('f',p.proowner))) a WHERE n.nspname=$1 AND a.grantee IN ((session_user::regrole)::OID,(recorded_rollout_operator_role()::regrole)::OID) AND (a.is_grantable OR p.proname::TEXT<>ALL($2::TEXT[])))").bind(schema).bind(vec!["issue_recorded_rollout_receipt","consume_recorded_rollout_receipt","revoke_recorded_rollout_receipt","recorded_rollout_admin_ledger"]).fetch_one(&mut *conn).await?;
        if extra_table_privilege || extra_function_grant {
            return Err(denied());
        }
        let sequence_access:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind='S' AND has_sequence_privilege(session_user,c.oid,'USAGE,SELECT,UPDATE'))").bind(schema).fetch_one(&mut *conn).await?;
        if sequence_access || runtime_roles.is_empty() {
            return Err(denied());
        }
        let owns_database:bool=sqlx::query_scalar("SELECT pg_has_role(session_user,datdba,'MEMBER') OR has_database_privilege(session_user,oid,'CREATE') FROM pg_database WHERE datname=current_database()").fetch_one(&mut *conn).await?;
        if owns_database {
            return Err(denied());
        }
        for runtime in runtime_roles {
            let valid:bool=sqlx::query_scalar("SELECT rolcanlogin AND NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolreplication AND NOT rolbypassrls AND NOT pg_has_role(oid,$1,'MEMBER') AND NOT has_schema_privilege(oid,$2,'CREATE') AND NOT has_schema_privilege(oid,'pg_catalog','CREATE') AND NOT pg_has_role(oid,(SELECT nspowner FROM pg_namespace WHERE nspname=$2),'MEMBER') AND NOT pg_has_role(oid,(SELECT datdba FROM pg_database WHERE datname=current_database()),'MEMBER') AND NOT has_database_privilege(oid,current_database(),'CREATE') FROM pg_roles WHERE rolname=$3").bind(&role).bind(schema).bind(runtime).fetch_optional(&mut *conn).await?.unwrap_or(false);
            if !valid {
                return Err(denied());
            }
            for name in functions {
                let can:bool=sqlx::query_scalar("SELECT has_function_privilege($1,p.oid,'EXECUTE') FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$2 AND p.proname=$3").bind(runtime).bind(schema).bind(name).fetch_one(&mut *conn).await?;
                if can {
                    return Err(denied());
                }
            }
            for table in [
                "recorded_rollout_offline_receipt",
                "recorded_rollout_receipt_terminal",
            ] {
                let can:bool=sqlx::query_scalar("SELECT has_any_column_privilege($1,c.oid,'SELECT') FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$2 AND c.relname=$3").bind(runtime).bind(schema).bind(table).fetch_one(&mut *conn).await?;
                if can {
                    return Err(denied());
                }
            }
            for table in tables {
                let can:bool=sqlx::query_scalar("SELECT has_table_privilege($1,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') OR has_any_column_privilege($1,c.oid,'INSERT,UPDATE,REFERENCES') FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$2 AND c.relname=$3").bind(runtime).bind(schema).bind(table).fetch_one(&mut *conn).await?;
                if can {
                    return Err(denied());
                }
            }
        }
        let ledger: Vec<(i64, Vec<u8>, bool)> =
            sqlx::query_as("SELECT version,checksum,success FROM recorded_rollout_admin_ledger()")
                .fetch_all(&mut *conn)
                .await?;
        for m in sqlx::migrate!("../../migrations").iter() {
            if !ledger
                .iter()
                .any(|(v, c, s)| *v == m.version && *s && c.as_slice() == m.checksum.as_ref())
            {
                return Err(denied());
            }
        }
        if ledger.len() != sqlx::migrate!("../../migrations").iter().count() {
            return Err(denied());
        }
        Ok(json!({"verified":true,"installation":status["installation"]}))
    }
}
