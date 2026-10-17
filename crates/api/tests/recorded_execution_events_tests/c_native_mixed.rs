//! A real cache page and Key-tainted Wait share one variable/output frontier.
use super::c_native_transport::{Boundary, Checkpoint};
use super::*;
use attune_common::{
    config::CacheAdmissionConfig,
    models::ExecutionExecutableSnapshot,
    repositories::{
        cache::{
            CacheGenerationRepository, CacheNamespacePolicy, CacheNamespaceRepository,
            CacheOwnerScope, CreateCacheNamespaceInput,
        },
        declared_input::DeclaredInputRepository,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
            PreparedInputSource,
        },
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        runtime_target::{ComponentTarget, RuntimeTarget},
        workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository},
        workflow_cache_iteration::WorkflowCacheIterationRepository,
    },
    secret_values::{redaction_marker, SecretValueInput},
};

pub(super) struct Cache {
    pub namespace: Id,
    pub reference: String,
    original: Id,
    replacement: Id,
}

#[tokio::test]
async fn mixed_native_key_wait_and_retired_cache_shared_parent_expires_only_original_contributor(
) -> Result<()> {
    for human in [false, true] {
        for boundary in [
            Boundary::Head,
            Boundary::Chunk,
            Boundary::Eof,
            Boundary::DownloadChunk,
            Boundary::DownloadEof,
            Boundary::Socket,
        ] {
            super::c_native_execution_wait::execution_wait(Some(Checkpoint::Mixed {
                boundary,
                human,
            }))
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn setup(fixture: &Fixture, pack: Id, pack_ref: &str) -> Result<Cache> {
    let reference = format!("mixed-native-{}", uuid::Uuid::new_v4().simple());
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        &fixture.ctx.pool,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(pack, Some(pack_ref.into())),
            namespace: reference.clone(),
            policy: CacheNamespacePolicy {
                freshness_target_seconds: 3600,
                ..Default::default()
            },
        },
        &CacheAdmissionConfig::default(),
    )
    .await?;
    let original = super::c_native_cache::generation(
        fixture,
        namespace.id,
        None,
        "mixed-original",
        "native-wait-original",
    )
    .await?;
    CacheGenerationRepository::promote(
        &fixture.ctx.pool,
        namespace.id,
        original,
        None,
        chrono::Utc::now() + chrono::Duration::hours(1),
    )
    .await?;
    let replacement = super::c_native_cache::generation(
        fixture,
        namespace.id,
        Some(original),
        "mixed-replacement",
        "wrong-current-cache-value",
    )
    .await?;
    Ok(Cache {
        namespace: namespace.id,
        reference,
        original,
        replacement,
    })
}

impl Cache {
    pub(super) async fn prepare_variable(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        execution: Id,
        workflow: Id,
        source: Id,
        pack: Id,
        graph: Id,
        graph_ref: &str,
        pin: &ExecutionExecutableSnapshot,
        key: &str,
    ) -> Result<(PreparedInputSource, Value)> {
        let binding = InvocationBinding::WorkflowTask {
            workflow_ref: graph_ref.into(),
            task_path: "/tasks/cached".into(),
            action_ref: pin.executable.action.r#ref.clone(),
        };
        let task = Admissions::select(
            tx,
            pack,
            binding,
            pin,
            &["standard".into()],
            RequestAttribution::ParentExecution {
                execution_id: execution,
            },
            RuntimeTarget::Component(ComponentTarget::Action, graph),
        )
        .await?;
        let prepared = Snapshots::prepare_cache_initial(
            tx,
            execution,
            "/workflow/tasks/2",
            &task,
            workflow,
            65536,
            &[],
            key,
        )
        .await?;
        let declarations = DeclaredInputRepository::bindings(tx, execution).await?;
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.source_path.ends_with("/tasks/2/params/token"))
            .unwrap();
        let mut session = Inputs::begin_render(
            tx,
            execution,
            &declaration.declaration_revision,
            &declaration.source_path,
            &declaration.expression,
            InputSitePurpose::Value,
        )
        .await?;
        Inputs::bind_cache_input(&mut session, &prepared)?;
        let rendered = Inputs::evaluate(tx, session).await?;
        assert!(rendered.value() == &json!("native-wait-original"));
        let site = InputSite {
            execution,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Value,
            path: "/cached".into(),
        };
        let destination = Inputs::prepare_destination(tx, &rendered, &site, key).await?;
        let iteration = WorkflowCacheIterationRepository::create(
            &mut **tx,
            prepared.initial_create_input().unwrap(),
        )
        .await?;
        Snapshots::bind_cache_frontier(tx, &prepared, iteration.id).await?;
        let receipt = Inputs::record_prepared(
            tx,
            &rendered,
            &destination,
            rendered.value(),
            rendered.selections(),
            key,
        )
        .await?;
        WorkflowExecutionRepository::update(&mut **tx,workflow,UpdateWorkflowExecutionInput {
            variables:Some(json!({"target_execution":source,"saved":redaction_marker(),"cached":redaction_marker()})),..Default::default()
        }).await?;
        let fields = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/cached".into(),
                value: rendered.value().clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&receipt.provenance())?),
            }],
            key,
        )?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            tx,
            "workflow_variables",
            execution,
            &fields,
        )
        .await?;
        let value = rendered.value().clone();
        let source = Inputs::prepare_source(tx, execution, &site, &receipt.origin(), key).await?;
        Ok((source, value))
    }
    pub(super) async fn retire(&self, fixture: &Fixture) -> Result<()> {
        CacheGenerationRepository::promote(
            &fixture.ctx.pool,
            self.namespace,
            self.replacement,
            Some(self.original),
            chrono::Utc::now() + chrono::Duration::hours(1),
        )
        .await?;
        Ok(())
    }
}
