//! Prospective controlled native producer and exact repository observations.
//! Only verified deployment enrollment + current operator IAM can create one.
use super::{
    artifact::{ArtifactRepository, CreateArtifactInput},
    execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
    execution_secret_value::SecretProvenanceRow,
    file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
    file_legacy_audit::LegacyAuditMember,
    file_origin::FileOriginRepository,
    identity::IdentityRepository,
    inherited_input::{InheritedInputRepository, InputEntityKind, InputSite, InputSitePurpose},
    native_generation_origin::{NativeMode, VerifiedNativeOrigin},
    runtime::{CreateWorkerInput, WorkerRepository},
    runtime_authority::{RuntimeAuthorityRepository, RuntimeCredentialBinding, RuntimeOwner},
    runtime_read::{RuntimeReadOrigin, RuntimeReadOwner},
    runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
    schema_contract::{BindingOwner, SchemaContractRepository},
    worker_delivery::WorkerDeliveryRepository,
    Create, FindByRef, Update,
};
use crate::{
    auth::jwt::{Claims, TokenType},
    delegation::DelegationAuthority,
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, ExecutionStatus,
        RetentionPolicyType, WorkerStatus, WorkerType,
    },
    rbac::{Action, AuthorizationContext, Resource},
    runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
    secret_values::{prepare_secret_values, redact_secret_parameters, SecretSource},
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSetupAccess {
    pub run_id: String,
    pub project: String,
    pub generation: Uuid,
    pub mode: NativeMode,
    pub maintenance_identity: i64,
    pub producer_action_ref: String,
    pub schema: String,
    pub database_name: String,
    pub database_role: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn access() -> NativeSetupAccess {
        NativeSetupAccess {
            run_id: "owned-run".into(),
            project: "eaccept-owned-run-native-blob".into(),
            generation: Uuid::new_v4(),
            mode: NativeMode::Blob,
            maintenance_identity: 7,
            producer_action_ref: "f_native_settlement.producer".into(),
            schema: "owned".into(),
            database_name: "owned".into(),
            database_role: "native_setup".into(),
        }
    }
    fn request(access: &NativeSetupAccess) -> NativeSetupRequest {
        NativeSetupRequest {
            protocol: 1,
            run_id: access.run_id.clone(),
            project: access.project.clone(),
            mode: access.mode,
            generation: Some(access.generation),
            operation: NativeSetupOperation::Setup,
            version_id: None,
        }
    }
    #[test]
    fn request_cannot_submit_members_sql_grants_or_foreign_generation() {
        let access = access();
        let mut request = request(&access);
        access.validate(&request).unwrap();
        request.generation = Some(Uuid::new_v4());
        assert!(access.validate(&request).is_err());
        for extra in ["member", "sql", "grants", "stopped"] {
            let mut value = json!({"protocol":1,"run_id":access.run_id,"project":access.project,"mode":"blob","operation":"setup"});
            value[extra] = json!(true);
            assert!(serde_json::from_value::<NativeSetupRequest>(value).is_err());
        }
    }
    #[test]
    fn runtime_integration_refresh_and_expired_tokens_are_not_maintenance_authority() {
        let access = access();
        let claims = Claims {
            sub: "7".into(),
            login: "acceptance.owned_run.maintenance".into(),
            iat: chrono::Utc::now().timestamp(),
            exp: chrono::Utc::now().timestamp() + 60,
            token_type: TokenType::Access,
            scope: None,
            metadata: None,
        };
        NativeFileSetupRepository::fresh(&access, &claims).unwrap();
        let mut expired = claims.clone();
        expired.exp = 0;
        assert!(NativeFileSetupRepository::fresh(&access, &expired).is_err());
        for token_type in [TokenType::Execution, TokenType::Worker, TokenType::Refresh] {
            let mut invalid = claims.clone();
            invalid.token_type = token_type;
            assert!(NativeFileSetupRepository::fresh(&access, &invalid).is_err());
        }
        let mut scoped = claims;
        scoped.scope = Some("integration_token".into());
        assert!(NativeFileSetupRepository::fresh(&access, &scoped).is_err());
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSetupRequest {
    pub protocol: u8,
    pub run_id: String,
    pub project: String,
    pub mode: NativeMode,
    pub generation: Option<Uuid>,
    pub operation: NativeSetupOperation,
    pub version_id: Option<i64>,
}
#[derive(Deserialize)]
pub enum NativeSetupOperation {
    #[serde(rename = "prepare-original-native-producer", alias = "setup")]
    Setup,
    #[serde(rename = "observe-original-native-custody", alias = "inspect")]
    Inspect,
}

#[derive(Serialize)]
pub struct NativeSetupResponse {
    pub protocol: u8,
    pub data: Value,
}

fn denied() -> Error {
    Error::PermissionDenied("Controlled native original setup/scope unavailable".into())
}

impl NativeSetupAccess {
    pub fn validate(&self, request: &NativeSetupRequest) -> Result<()> {
        if request.protocol != 1
            || request.run_id != self.run_id
            || request.project != self.project
            || request.mode != self.mode
            || request.generation.is_some_and(|g| g != self.generation)
            || self.generation.is_nil()
            || self.maintenance_identity <= 0
            || self.run_id.is_empty()
            || self.run_id.len() > 20
            || !self.run_id.as_bytes()[0].is_ascii_lowercase()
            || !self
                .run_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !self
                .project
                .starts_with(&format!("eaccept-{}-", self.run_id))
            || self.project.len() > 128
            || !self
                .project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            || self.schema.is_empty()
            || self.schema.len() > 63
            || !self
                .schema
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(denied());
        }
        Ok(())
    }
}

pub struct NativeFileSetupRepository;
impl NativeFileSetupRepository {
    async fn scope(conn: &mut PgConnection, access: &NativeSetupAccess) -> Result<()> {
        let (database, role, superuser): (String, String, String) = sqlx::query_as(
            "SELECT current_database(),current_user,current_setting('is_superuser')",
        )
        .fetch_one(&mut *conn)
        .await?;
        if database != access.database_name
            || role != access.database_role
            || superuser != "off"
            || role == "postgres"
        {
            return Err(denied());
        }
        // A private context does not make an administrative login safe. Require
        // direct object privileges on the owned database, not inherited roles or
        // cluster-wide administration/file/process capabilities.
        let elevated: bool = sqlx::query_scalar(
            "SELECT r.rolsuper OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication OR r.rolbypassrls OR EXISTS(SELECT 1 FROM pg_auth_members m WHERE m.member=r.oid) FROM pg_roles r WHERE r.rolname=current_user",
        )
        .fetch_one(&mut *conn)
        .await?;
        if elevated {
            return Err(denied());
        }
        sqlx::query("SELECT set_config('search_path',quote_ident($1),true),set_config('statement_timeout','30s',true)")
            .bind(&access.schema).execute(conn).await?;
        Ok(())
    }

    async fn authorize(
        conn: &mut PgConnection,
        access: &NativeSetupAccess,
        claims: &Claims,
        write: bool,
    ) -> Result<()> {
        Self::fresh(access, claims)?;
        let authority =
            DelegationAuthority::load_for_share(conn, access.maintenance_identity).await?;
        if !authority.manages_permissions()
            || !authority.allows(
                Resource::Retention,
                if write { Action::Update } else { Action::Read },
                AuthorizationContext::new(access.maintenance_identity),
            )
        {
            return Err(denied());
        }
        let identity = IdentityRepository::find_by_id_for_share(conn, access.maintenance_identity)
            .await?
            .ok_or_else(denied)?;
        if identity.login != claims.login {
            return Err(denied());
        }
        Ok(())
    }
    fn fresh(access: &NativeSetupAccess, claims: &Claims) -> Result<()> {
        if claims.token_type != TokenType::Access
            || claims.scope.is_some()
            || claims.exp <= chrono::Utc::now().timestamp()
            || claims.sub != access.maintenance_identity.to_string()
            || claims.login != format!("acceptance.{}.maintenance", access.run_id.replace('-', "_"))
            || claims
                .admitted_execution_credential()
                .map_err(|_| denied())?
                .is_some()
        {
            return Err(denied());
        }
        Ok(())
    }

    async fn original(
        conn: &mut PgConnection,
        access: &NativeSetupAccess,
    ) -> Result<Option<Value>> {
        let row:Option<(String,String,String,i64,Value)>=sqlx::query_as("SELECT g.run_id,g.project,g.mode,g.operator,jsonb_build_object('member',n.member,'execution_id',n.execution,'admission_id',n.admission,'worker_id',n.worker,'source_receipt',n.source_receipt,'artifact_id',n.artifact,'artifact_version',n.artifact_version,'scope',g.scope,'action_ref',g.action_ref,'protocol','controlled_native_v1') FROM file_native_generation g LEFT JOIN file_native_operation n ON n.generation=g.generation WHERE g.generation=$1")
            .bind(access.generation).fetch_optional(&mut *conn).await?;
        let Some((run, project, mode, operator, mut value)) = row else {
            return Ok(None);
        };
        if run != access.run_id
            || project != access.project
            || mode != access.mode.as_str()
            || operator != access.maintenance_identity
            || value["action_ref"] != access.producer_action_ref
            || value["member"].is_null()
        {
            return Err(denied());
        }
        let version = value["member"]["version"].as_i64().ok_or_else(denied)?;
        // API identifiers are immutable generation records, not a by-ref rebound.
        value["version_id"] = json!(version);
        Ok(Some(value))
    }

    pub async fn existing(
        pool: &PgPool,
        access: &NativeSetupAccess,
        claims: &Claims,
    ) -> Result<Option<Value>> {
        let mut tx = pool.begin().await?;
        Self::scope(&mut tx, access).await?;
        Self::authorize(&mut tx, access, claims, true).await?;
        let result = Self::original(&mut tx, access).await?;
        if let Some(value) = &result {
            let available:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=$1 AND NOT metadata_gone AND cleanup_group IS NULL)")
                .bind(value["member"]["version"].as_i64().ok_or_else(denied)?).fetch_one(&mut *tx).await?;
            if !available {
                return Err(denied());
            }
        }
        tx.commit().await?;
        Self::fresh(access, claims)?;
        Ok(result)
    }

    pub async fn setup(
        pool: &PgPool,
        access: &NativeSetupAccess,
        claims: &Claims,
        origin: VerifiedNativeOrigin,
        encryption_key: &str,
    ) -> Result<Value> {
        if origin.generation != access.generation
            || origin.mode != access.mode
            || encryption_key.len() < 32
        {
            return Err(denied());
        }
        origin.ensure_fresh()?;
        let mut tx = pool.begin().await?;
        Self::scope(&mut tx, access).await?;
        Evidence::lock_capture(&mut tx).await?;
        Self::authorize(&mut tx, access, claims, true).await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtext('file_native_generation'),hashtext($1))",
        )
        .bind(access.generation.to_string())
        .execute(&mut *tx)
        .await?;
        if let Some(original) = Self::original(&mut tx, access).await? {
            if original["scope"] != origin.scope {
                return Err(denied());
            }
            tx.commit().await?;
            Self::fresh(access, claims)?;
            return Ok(original);
        }
        let live =
            super::action::ActionRepository::find_by_ref(&mut *tx, &access.producer_action_ref)
                .await?
                .ok_or_else(denied)?;
        let snapshot=super::execution_runtime_admission::ExecutionAdmissionRepository::resolve_current_snapshot(
            &mut tx,live.pack,RuntimeTarget::Component(ComponentTarget::Action,live.id),
            &InvocationBinding::Action{action_ref:live.r#ref.clone()},
        ).await?;
        let action = &snapshot.executable.action;
        if action
            .param_schema
            .as_ref()
            .and_then(|v| v.get("native_source"))
            .and_then(|v| v.get("secret"))
            .and_then(Value::as_bool)
            != Some(true)
        {
            return Err(denied());
        }
        let target = RuntimeTargetRepository::resolve(
            &mut tx,
            RuntimeTarget::Component(ComponentTarget::Action, action.id),
            access.maintenance_identity,
        )
        .await?
        .ok_or_else(denied)?;
        let human =
            DelegationAuthority::load_for_share(&mut tx, access.maintenance_identity).await?;
        if !human.allows(Resource::Actions, Action::Execute, target.context().clone())
            || !human.allows(
                Resource::Workers,
                Action::Create,
                AuthorizationContext::new(access.maintenance_identity),
            )
        {
            return Err(denied());
        }
        human
            .require_refs_for_share(&mut tx, &["standard".into()])
            .await?;
        let selected = super::execution_runtime_admission::ExecutionAdmissionRepository::select(
            &mut tx,
            action.pack,
            InvocationBinding::Action {
                action_ref: action.r#ref.clone(),
            },
            &snapshot,
            &["standard".into()],
            RequestAttribution::Human {
                actor: HumanAttribution {
                    identity_id: access.maintenance_identity,
                    login: claims.login.clone(),
                },
            },
            RuntimeTarget::Component(ComponentTarget::Action, action.id),
        )
        .await?;
        // Only the nonce is selected by the authenticated prospective deployment
        // identity. Grants/owner/release/attribution remain the actual selection.
        // This happens BEFORE the first admission or input seal, never by UPDATE.
        let mut authority = selected.authority().clone();
        authority.attempt = origin.attempt;
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_authority_execution_fence WHERE attempt=$1)",
        )
        .bind(origin.attempt)
        .fetch_one(&mut *tx)
        .await?;
        if used {
            return Err(denied());
        }
        sqlx::query("INSERT INTO file_native_generation(generation,run_id,project,mode,operator,native_instance,worker_instance,execution_attempt,scope,origin,origin_digest,action_ref) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
            .bind(access.generation).bind(&access.run_id).bind(&access.project).bind(access.mode.as_str()).bind(access.maintenance_identity)
            .bind(origin.native_instance).bind(origin.worker_instance).bind(origin.attempt).bind(&origin.scope).bind(&origin.evidence).bind(&origin.identity_digest).bind(&access.producer_action_ref).execute(&mut *tx).await?;
        let producer = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(action.id),
                action_ref: action.r#ref.clone(),
                executor: Some(access.maintenance_identity),
                permission_set_refs: vec!["standard".into()],
                status: ExecutionStatus::Requested,
                config: None,
                ..Default::default()
            },
            &snapshot,
        )
        .await?;
        let admission = RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(producer.id),
            &authority,
        )
        .await?;
        let schema = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot).await?;
        schema.validate()?;
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, producer.id, &schema)
            .await?;
        if SchemaContractRepository::find(&mut tx, BindingOwner::Execution, producer.id)
            .await?
            .as_ref()
            != Some(&schema)
        {
            return Err(denied());
        }
        let input = json!({"native_source":format!("owned native source {} {}",access.run_id,access.generation)});
        let (public, fields) = redact_secret_parameters(input, action.param_schema.as_ref());
        if fields.len() != 1 || fields[0].json_path != "/native_source" {
            return Err(denied());
        }
        let fields = prepare_secret_values(fields, encryption_key)?;
        ExecutionRepository::initialize_config_with_conn(
            &mut tx,
            producer.id,
            Some(admission.id),
            &snapshot,
            Some(public),
            &fields,
        )
        .await?;
        super::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
            &mut tx,
            "execution",
            producer.id,
            access.maintenance_identity,
        )
        .await?;
        super::accepted_input::AcceptedInputRepository::seal_execution(
            &mut tx,
            producer.id,
            encryption_key,
        )
        .await?;
        let worker = WorkerRepository::create(
            &mut *tx,
            CreateWorkerInput {
                name: format!("Controlled native {}", access.generation),
                worker_type: WorkerType::Local,
                runtime: None,
                host: None,
                port: None,
                status: Some(WorkerStatus::Active),
                capabilities: None,
                meta: Some(json!({"native_generation":access.generation})),
            },
        )
        .await?;
        ExecutionRepository::update(
            &mut *tx,
            producer.id,
            UpdateExecutionInput {
                worker: Some(worker.id),
                status: Some(ExecutionStatus::Running),
                ..Default::default()
            },
        )
        .await?;
        let credential = RuntimeCredentialBinding {
            execution_id: producer.id,
            admission_id: admission.id,
            sponsor_identity_id: admission.authority.owner.sponsor_identity_id,
            action_ref: action.r#ref.clone(),
            attempt: origin.attempt,
        };
        let runtime =
            RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &credential).await?;
        let delivery = WorkerDeliveryRepository::register(
            &mut tx,
            producer.id,
            worker.id,
            origin.worker_instance,
            chrono::Utc::now() + chrono::Duration::hours(1),
        )
        .await?;
        let site = InputSite {
            execution: producer.id,
            kind: InputEntityKind::ExecutionConfig,
            purpose: InputSitePurpose::Value,
            path: "/native_source".into(),
        };
        let original = SecretSource::ParameterSchema {
            path: site.path.clone(),
        }
        .provenance();
        let source = super::accepted_input::AcceptedInputRepository::prepare_source(
            &mut tx,
            producer.id,
            &site,
            &original,
            encryption_key,
        )
        .await?;
        let receipt = InheritedInputRepository::receipt_context(
            &mut tx,
            source.site(),
            &source.origin(),
            encryption_key,
        )
        .await?;
        // Opaque original source contexts are prepared before destination locks.
        let artifact = ArtifactRepository::create(
            &mut *tx,
            CreateArtifactInput {
                r#ref: format!("{}.native_{}", action.pack_ref, access.generation.simple()),
                scope: crate::models::OwnerType::Pack,
                owner: action.pack_ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::General,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: None,
                data: None,
            },
        )
        .await?;
        let artifact_target = RuntimeTargetRepository::resolve(
            &mut tx,
            RuntimeTarget::Artifact(artifact.id),
            access.maintenance_identity,
        )
        .await?
        .ok_or_else(denied)?;
        if !human.allows(
            Resource::Artifacts,
            Action::Create,
            artifact_target.context().clone(),
        ) {
            return Err(denied());
        }
        let object = access
            .mode
            .blob()
            .then(|| format!("artifacts/{}/native/{}", artifact.id, origin.operation));
        let staging = access
            .mode
            .shared()
            .then(|| format!("native/{}/{}.stage", access.generation, origin.operation));
        let query=format!("INSERT INTO artifact_version(artifact,version,execution,content_type,body_state,object_key,file_path,meta) SELECT $1,COALESCE(MAX(version),0)+1,$2,'application/octet-stream','pending',$3,$4,$5 FROM artifact_version WHERE artifact=$1 RETURNING {}",crate::models::artifact_version::SELECT_COLUMNS);
        let version: crate::models::ArtifactVersion = sqlx::query_as(&query)
            .bind(artifact.id)
            .bind(producer.id)
            .bind(&object)
            .bind(&staging)
            .bind(json!({"f_native_run":access.run_id,"f_native_generation":access.generation}))
            .fetch_one(&mut *tx)
            .await?;
        if version.id > 9_007_199_254_740_991 {
            return Err(denied());
        }
        let effect = WorkerDeliveryRepository::process_effect(&runtime, &delivery)?;
        WorkerDeliveryRepository::bind(
            &mut tx,
            &delivery,
            version.id,
            None,
            512 * 1024,
            0,
            Action::Create,
            &effect,
        )
        .await?;
        let target = RuntimeTargetRepository::resolve(
            &mut tx,
            RuntimeTarget::Artifact(artifact.id),
            access.maintenance_identity,
        )
        .await?
        .ok_or_else(denied)?;
        // A pending sink is not an acquired source read. The exact accepted
        // input receipt is frozen below as a reservation source, just as in the
        // real native custody path, not invented as a RuntimeReadParent.
        let read = RuntimeReadOrigin {
            target: serde_json::from_value(
                json!({"kind":"artifact_version","artifact":artifact.id,"version":version.id,"execution":producer.id,"owner":RuntimeReadOwner::of(&target)}),
            )?,
            provenance: vec![],
            parents: vec![],
        };
        let row = SecretProvenanceRow {
            entity_type: site.kind.as_str().into(),
            entity_id: producer.id,
            json_path: site.path.clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance())?),
        };
        FileOriginRepository::reserve(
            &mut tx,
            version.id,
            admission.id,
            producer.id,
            origin.attempt,
            admission.authority.owner.sponsor_identity_id,
            &read,
            &[row],
        )
        .await?;
        let retained = FileOriginRepository::find(&mut tx, version.id)
            .await?
            .ok_or_else(denied)?;
        if retained.origin_read > 9_007_199_254_740_991 {
            return Err(denied());
        }
        let member = LegacyAuditMember {
            operation: origin.operation,
            version: version.id,
            root_read: retained.origin_read,
            object_key: object,
            provider_version: None,
            staging_path: staging,
            execution_attempt: Some(origin.attempt),
            worker_instance: Some(origin.worker_instance),
            native_instance: Some(origin.native_instance),
        };
        let member_json = serde_json::to_value(&member)?;
        let source_receipt = match receipt.origin() {
            crate::secret_provenance::SecretOrigin::Entity { entity_id, .. } => entity_id,
            _ => return Err(denied()),
        };
        // Registration is a real prospective protocol, never legacy_unknown.
        sqlx::query("INSERT INTO file_native_operation(operation,generation,version,execution,admission,worker,source_receipt,member,member_digest,artifact,artifact_version) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(origin.operation).bind(access.generation).bind(version.id).bind(producer.id).bind(admission.id).bind(worker.id)
            .bind(source_receipt).bind(&member_json).bind(digest(&member_json).map_err(|_| denied())?).bind(artifact.id).bind(version.version).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO file_evidence_put(attempt,owner,protocol,execution,execution_attempt,worker_instance,native_instance) VALUES($1,$2,'controlled_native_v1',$3,$4,$5,$6)")
            .bind(origin.operation).bind(version.id).bind(producer.id).bind(origin.attempt).bind(origin.worker_instance).bind(origin.native_instance).execute(&mut *tx).await?;
        source.ensure_fresh()?;
        receipt.ensure_fresh()?;
        effect.ensure_fresh()?;
        selected.ensure_fresh_on(&mut tx).await?;
        origin.ensure_fresh()?;
        Self::fresh(access, claims)?;
        let result = json!({"member":member,"artifact_id":artifact.id,"artifact_version":version.version,"execution_id":producer.id,"admission_id":admission.id,
            "worker_id":worker.id,"scope":origin.scope,"source_receipt":source_receipt,"protocol":"controlled_native_v1"});
        tx.commit().await?;
        source.ensure_fresh()?;
        receipt.ensure_fresh()?;
        effect.ensure_fresh()?;
        origin.ensure_fresh()?;
        Self::fresh(access, claims)?;
        Ok(result)
    }

    pub async fn inspect(
        pool: &PgPool,
        access: &NativeSetupAccess,
        claims: &Claims,
        version: Option<i64>,
    ) -> Result<Value> {
        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        Self::scope(&mut tx, access).await?;
        Self::authorize(&mut tx, access, claims, false).await?;
        let original = Self::original(&mut tx, access).await?.ok_or_else(denied)?;
        let id = original["member"]["version"].as_i64().ok_or_else(denied)?;
        if version.is_some_and(|v| v != id) {
            return Err(denied());
        }
        let (owners,pins,puts,object_done,staging_done):(i64,i64,i64,Option<bool>,Option<bool>)=sqlx::query_as("SELECT (SELECT count(*) FROM file_evidence_owner WHERE version=$1),(SELECT count(*) FROM file_evidence_pin WHERE owner=$1),(SELECT count(*) FROM file_evidence_put WHERE owner=$1),(SELECT object_done FROM file_evidence_owner WHERE version=$1),(SELECT staging_done FROM file_evidence_owner WHERE version=$1)")
            .bind(id).fetch_one(&mut *tx).await?;
        let audit:Option<Uuid>=sqlx::query_scalar("SELECT a.id FROM file_legacy_audit_member m JOIN file_legacy_audit a ON a.id=m.audit WHERE m.operation=$1 AND m.version=$2 ORDER BY (a.state='manual_required'),a.id DESC LIMIT 1")
            .bind(original["member"]["operation"].as_str().and_then(|s|Uuid::parse_str(s).ok()).ok_or_else(denied)?).bind(id).fetch_optional(&mut *tx).await?;
        let mut observation = json!({"member":original["member"],"scope":original["scope"],"evidence_owner_count":owners,"pins":pins,"unresolved_operations":puts,"object_done":object_done,"staging_done":staging_done,"audit":null});
        if let Some(audit) = audit {
            let status:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'state',a.state,'plan',a.plan,'plan_matches_original',(a.plan->'members'=$2::jsonb AND a.plan->'scope'=$3::jsonb),'receipt_counts',(SELECT COALESCE(jsonb_object_agg(kind,n),'{}'::jsonb) FROM (SELECT kind,count(*) n FROM file_legacy_audit_evidence WHERE audit=a.id GROUP BY kind) q),'controller_receipts',(SELECT COALESCE(jsonb_agg(jsonb_build_object('identity',q.identity,'operation',q.proof->>'operation','request_digest',q.proof->>'request_digest','verification_key_sha256',q.proof->>'verification_key_sha256')),'[]'::jsonb) FROM (SELECT identity,proof FROM file_legacy_audit_evidence WHERE audit=a.id AND kind='controller' ORDER BY identity LIMIT 64) q)) FROM file_legacy_audit a WHERE a.id=$1")
                .bind(audit).bind(json!([original["member"]])).bind(&original["scope"]).fetch_one(&mut *tx).await?;
            observation["audit"] = status;
        }
        tx.commit().await?;
        Self::fresh(access, claims)?;
        Ok(observation)
    }
}
