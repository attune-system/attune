//! Owned, bounded historical READ TARGETS, not independently executed workloads.
//!
//! The selected run and explicit security probes use real API acceptance after
//! one real pack deployment. Historical rows retain its genuine executable and
//! released schema, but never copy runtime authority, receipts, signatures,
//! provenance, file origins, or credentials. Historical log versions are metadata
//! only; selected/probe versions have tiny synthetic bodies. This fixture cannot
//! certify write throughput, protected-log transport, or log-body capacity.
use super::*;
use attune_common::{
    models::Execution,
    repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository,
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
        schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository},
        FindById,
    },
};

const MAX_BATCH: usize = 1000;
const MAX_RETAINED: usize = 604_800;

#[derive(Clone, Copy)]
pub(super) struct VersionSelector {
    pub artifact: i64,
    pub id: i64,
    pub version: i32,
}

impl VersionSelector {
    pub fn download_path(self) -> String {
        format!(
            "/api/v1/artifacts/{}/versions/{}/download?version_id={}",
            self.artifact, self.version, self.id
        )
    }
}

#[derive(Clone, Copy)]
pub(super) struct RunSelectors {
    pub execution: i64,
    pub versions: [VersionSelector; 2],
}

/// One owned context, one admitted template, fixed selectors, and a counter.
/// Application memory retains at most `batch_size` historical IDs at a time.
pub(super) struct RetainedHistory {
    context: TestContext,
    template: Execution,
    binding: ActionSchemaBinding,
    selected: RunSelectors,
    retained: usize,
    batch_size: usize,
}

impl RetainedHistory {
    pub async fn new(batch_size: usize) -> Result<Self> {
        if !(1..=MAX_BATCH).contains(&batch_size) {
            return Err("retained fixture batch size must be 1..=1000".into());
        }
        let context = TestContext::new_without_authorization_metadata_cache()
            .await?
            .with_auth()
            .await?;
        let prepared = Self::prepare(&context).await;
        match prepared {
            Ok((template, binding, selected)) => Ok(Self {
                context,
                template,
                binding,
                selected,
                retained: 1,
                batch_size,
            }),
            Err(error) => {
                let cleanup = context.cleanup().await;
                if let Err(cleanup) = cleanup {
                    return Err(format!(
                        "retained fixture setup failed: {error}; cleanup failed: {cleanup}"
                    )
                    .into());
                }
                Err(error)
            }
        }
    }

    async fn prepare(ctx: &TestContext) -> Result<(Execution, ActionSchemaBinding, RunSelectors)> {
        let action_ref = action(ctx).await?;
        let execution = execution(ctx, &action_ref).await?;
        let template = ExecutionRepository::find_by_id(&ctx.pool, execution)
            .await?
            .ok_or("accepted execution missing")?;
        let binding = SchemaContractRepository::find_with_executor(
            &ctx.pool,
            BindingOwner::Execution,
            execution,
        )
        .await?
        .ok_or("accepted execution binding missing")?;
        binding.validate()?;
        if template.config != Some(json!({}))
            || template.executable_snapshot.is_none()
            || template.executor.is_none()
        {
            return Err(
                "historical template must have genuine pinned metadata and empty nonsecret inputs"
                    .into(),
            );
        }
        let mut versions = Vec::with_capacity(2);
        for stream in ["stdout", "stderr"] {
            let artifact = ArtifactRepository::create(
                &ctx.pool,
                CreateArtifactInput {
                    r#ref: format!("{action_ref}.{stream}.log"),
                    scope: OwnerType::Action,
                    owner: action_ref.clone(),
                    r#type: ArtifactType::FileText,
                    visibility: ArtifactVisibility::Private,
                    classification: ArtifactClassification::RuntimeLog,
                    retention_policy: RetentionPolicyType::Versions,
                    retention_limit: i32::try_from(MAX_RETAINED + 16)?,
                    name: None,
                    description: None,
                    content_type: Some("application/json".into()),
                    data: None,
                },
            )
            .await?;
            versions.push(Self::write_version(ctx, artifact.id, execution, "selected").await?);
        }
        let selected = RunSelectors {
            execution,
            versions: versions
                .try_into()
                .map_err(|_| "two log selectors required")?,
        };
        Ok((template, binding, selected))
    }

    pub fn context(&self) -> &TestContext {
        &self.context
    }

    pub fn selected(&self) -> RunSelectors {
        self.selected
    }

    pub fn retained(&self) -> usize {
        self.retained
    }

    pub async fn grow_to(&mut self, target: usize) -> Result<()> {
        if target < self.retained || target > MAX_RETAINED {
            return Err("retained target must grow monotonically and be at most 604800".into());
        }
        while self.retained < target {
            let count = self.batch_size.min(target - self.retained);
            let mut tx = self.context.pool.begin().await?;
            match self.write_batch(&mut tx, count).await {
                Ok(()) => {
                    tx.commit().await?;
                    self.retained += count;
                }
                Err(error) => {
                    tx.rollback().await?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    async fn write_batch(&self, tx: &mut sqlx::PgConnection, count: usize) -> Result<()> {
        FileEvidenceRetentionRepository::lock_capture(tx).await?;
        let snapshot = self
            .template
            .executable_snapshot
            .as_ref()
            .ok_or("template snapshot missing")?;
        let stored = SchemaContractRepository::find(tx, BindingOwner::Execution, self.template.id)
            .await?
            .ok_or("template binding no longer available")?;
        if stored != self.binding {
            return Err("historical schema template changed".into());
        }
        let mut artifacts = self.selected.versions.map(|version| version.artifact);
        artifacts.sort_unstable();
        for artifact in artifacts {
            // Enter sorted version advisory admission BEFORE any parent row
            // lock, including RuntimeTarget::resolve's FOR SHARE owner probe.
            // Otherwise a concurrent writer can hold the advisory lock while
            // its INSERT trigger waits for a parent row already shared by us.
            ArtifactRepository::find_by_id_for_update(&mut *tx, artifact)
                .await?
                .ok_or("historical artifact missing")?;
        }
        let caller = self.template.executor.ok_or("template executor missing")?;
        for version in self.selected.versions {
            let target = RuntimeTargetRepository::resolve(
                tx,
                RuntimeTarget::Artifact(version.artifact),
                caller,
            )
            .await?
            .ok_or("captured artifact ownership unavailable")?;
            if target.owner_id() != self.template.action
                || target.pack_id() != Some(snapshot.executable.action.pack)
            {
                return Err(
                    "historical fixture requires surviving captured action/pack ownership".into(),
                );
            }
        }
        // Fixture-only batch SQL on this context's owned clone. These terminal
        // read targets are deliberately not dispatchable accepted invocations.
        let snapshot = serde_json::to_value(snapshot)?;
        let ids: Vec<i64> = sqlx::query_scalar(
            "INSERT INTO execution (action,action_ref,pack_release,pack_release_digest,executable_snapshot,executor,config,status) \
             SELECT $1,$2,$3,$4,$5,$6,'{}'::jsonb,'completed'::execution_status_enum FROM generate_series(1,$7::bigint) RETURNING id",
        ).bind(self.template.action).bind(&self.template.action_ref)
            .bind(self.template.pack_release).bind(&self.template.pack_release_digest)
            .bind(&snapshot).bind(caller).bind(i64::try_from(count)?).fetch_all(&mut *tx).await?;
        if ids.len() != count {
            return Err("historical execution batch was incomplete".into());
        }
        // Copy only the validated released CONTRACT, never an authority envelope
        // or source proof. Every destination has the exact template snapshot.
        let body = serde_json::to_value(&stored)?;
        let bound = sqlx::query(
            "INSERT INTO runtime_schema_binding (entity_kind,entity_id,contract,binding_digest) \
             SELECT 'execution',e.id,original.contract,original.binding_digest \
             FROM execution e CROSS JOIN runtime_schema_binding original \
             WHERE e.id=ANY($1) AND e.executable_snapshot=$4 AND original.entity_kind='execution' \
             AND original.entity_id=$2 AND original.contract=$3",
        )
        .bind(&ids)
        .bind(self.template.id)
        .bind(&body)
        .bind(&snapshot)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if bound != count as u64 {
            return Err("historical schema batch did not match its pinned executables".into());
        }
        for artifact in artifacts {
            let linked = sqlx::query(
                "INSERT INTO artifact_version (artifact,version,execution,content_type) \
                 SELECT $1,(base.version+n.ordinality)::integer,n.id,'application/json' \
                 FROM unnest($2::bigint[]) WITH ORDINALITY n(id,ordinality) CROSS JOIN \
                 (SELECT COALESCE(MAX(version),0) AS version FROM artifact_version WHERE artifact=$1) base",
            ).bind(artifact).bind(&ids).execute(&mut *tx).await?.rows_affected();
            if linked != count as u64 {
                return Err("historical log batch was incomplete".into());
            }
        }
        Ok(())
    }

    /// Security probes are genuine new acceptances, not historical copies.
    pub async fn accept_probe(&mut self, marker: &str) -> Result<RunSelectors> {
        if self.retained == MAX_RETAINED {
            return Err("retained fixture is at its declared ceiling".into());
        }
        let execution = execution(&self.context, &self.template.action_ref).await?;
        let stdout = self.append_version(execution, 0, marker).await?;
        let stderr = self.append_version(execution, 1, marker).await?;
        self.retained += 1;
        Ok(RunSelectors {
            execution,
            versions: [stdout, stderr],
        })
    }

    pub async fn append_version(
        &self,
        execution: i64,
        stream: usize,
        marker: &str,
    ) -> Result<VersionSelector> {
        let artifact = self
            .selected
            .versions
            .get(stream)
            .ok_or("stream must be stdout or stderr")?
            .artifact;
        Self::write_version(&self.context, artifact, execution, marker).await
    }

    async fn write_version(
        ctx: &TestContext,
        artifact: i64,
        execution: i64,
        marker: &str,
    ) -> Result<VersionSelector> {
        if marker.len() > 128 {
            return Err("fixture marker exceeds its byte budget".into());
        }
        let version = ArtifactVersionRepository::create(
            &ctx.pool,
            CreateArtifactVersionInput {
                artifact,
                execution: Some(execution),
                content_type: Some("application/json".into()),
                content: None,
                content_json: Some(json!({"execution":execution,"marker":marker})),
                file_path: None,
                meta: None,
                created_by: None,
            },
        )
        .await?;
        Ok(VersionSelector {
            artifact,
            id: version.id,
            version: version.version,
        })
    }

    pub async fn cleanup(self) -> Result<()> {
        self.context.cleanup().await
    }
}
