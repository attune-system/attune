//! Internal source-transfer contracts; no public enqueue or readiness evidence.
use super::*;
use attune_common::{
    models::{Execution, OwnerType},
    repositories::{
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        key::{CreateKeyInput, KeyRepository, UpdateKeyInput},
        native_workflow_materialization::NativeWorkflowMaterializationRepository as Variables,
        queue_configuration_input::{
            QueueConfigurationInput, QueueConfigurationInputRepository as Fixed,
        },
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        runtime_authority::RuntimeCredentialBinding,
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{CreateWorkflowExecutionInput, WorkflowExecutionRepository},
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{
        prepare_secret_values, SecretValueInput, ENTITY_EXECUTION_CONFIG,
        ENTITY_QUEUE_ITEM_PAYLOAD, ENTITY_WORKFLOW_VARIABLES,
    },
};

#[tokio::test]
async fn admitted_component_receipts_transfer_without_human_key_rights_and_survive_completion() {
    super::mixed_queue_case(false, false, true).await;
}

pub(super) fn admit_source_effect(policy: &mut RuntimePolicy) {
    let reference = format!("{}.transfer", policy.owner.pack_ref);
    let definition: PermissionSetDefinition = serde_json::from_value(
        json!({"ref":reference,"grants":[{"resource":"queue_items","actions":["create"]}]}),
    )
    .unwrap();
    let compiled = definition.compile(&policy.owner, &[]).unwrap();
    for invocation in &mut policy.invocations {
        if matches!(invocation.binding, InvocationBinding::Action { .. }) {
            invocation
                .permission_sets
                .insert(reference.clone(), compiled.clone());
        }
    }
}

pub(super) struct Source {
    pub(super) execution: Execution,
    pub(super) admission: i64,
    credential: RuntimeCredentialBinding,
    origin: SecretOrigin,
    same_value_other_site: SecretOrigin,
}
impl Source {
    pub(super) async fn complete_and_remove(&self, conn: &mut sqlx::PgConnection) {
        use attune_common::repositories::{execution::UpdateExecutionInput, Delete};
        ExecutionRepository::update(
            &mut *conn,
            self.execution.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            Authority::validate_execution_credential(conn, &self.credential)
                .await
                .is_err(),
            "a completed producer cannot issue another transfer"
        );
        ExecutionRepository::delete(&mut *conn, self.execution.id)
            .await
            .unwrap();
    }
    pub(super) async fn prepare(
        conn: &mut sqlx::PgConnection,
        snapshot: &ExecutionExecutableSnapshot,
        policy: &RuntimePolicy,
        actor: i64,
        value: &str,
    ) -> Self {
        let key = KeyRepository::create(
            &mut *conn,
            CreateKeyInput {
                local_ref: format!("transfer_{}", uuid::Uuid::new_v4().simple()),
                owner_type: OwnerType::Pack,
                owner_identity: None,
                owner_pack: Some(policy.owner.pack_id),
                owner_pack_ref: Some(policy.owner.pack_ref.clone()),
                owner_action: None,
                owner_action_ref: None,
                owner_sensor: None,
                owner_sensor_ref: None,
                name: format!("Owned transfer fixture {}", uuid::Uuid::new_v4().simple()),
                encrypted: false,
                encryption_key_hash: None,
                value: json!(value),
            },
        )
        .await
        .unwrap();
        KeyRepository::update(
            &mut *conn,
            key.id,
            UpdateKeyInput {
                encrypted: Some(true),
                encryption_key_hash: Some(
                    attune_common::crypto::keystore::fingerprint(QUEUE_KEY, key.id).unwrap(),
                ),
                value: Some(
                    attune_common::crypto::keystore::encrypt_json(&json!(value), QUEUE_KEY, key.id)
                        .unwrap(),
                ),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let refs = vec![format!("{}.transfer", policy.owner.pack_ref)];
        let admitted = Admission::select(
            conn,
            policy.owner.pack_id,
            InvocationBinding::Action {
                action_ref: snapshot.executable.action.r#ref.clone(),
            },
            snapshot,
            &refs,
            RequestAttribution::Human {
                actor: HumanAttribution {
                    identity_id: actor,
                    login: format!("source-{actor}"),
                },
            },
            RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
        )
        .await
        .unwrap();
        let (config, fields) = attune_common::secret_values::redact_secret_parameters(
            json!({"value":value,"key_ref":key.r#ref,"condition_value":"false"}),
            snapshot.executable.action.param_schema.as_ref(),
        );
        let fields = prepare_secret_values(fields, QUEUE_KEY).unwrap();
        let execution = ExecutionRepository::create_pinned(
            &mut *conn,
            CreateExecutionInput {
                action: Some(snapshot.executable.action.id),
                action_ref: snapshot.executable.action.r#ref.clone(),
                executor: Some(actor),
                status: ExecutionStatus::Running,
                permission_set_refs: refs,
                config: Some(config),
                ..Default::default()
            },
            snapshot,
        )
        .await
        .unwrap();
        let record = admitted
            .attach(conn, RuntimeOwner::Execution(execution.id))
            .await
            .unwrap();
        let schema = Schemas::materialize_action_pair(conn, snapshot)
            .await
            .unwrap();
        Schemas::attach(conn, BindingOwner::Execution, execution.id, &schema)
            .await
            .unwrap();
        Contributors::capture_human(conn, "execution", execution.id, actor)
            .await
            .unwrap();
        Secrets::upsert_many_with_conn(&mut *conn, ENTITY_EXECUTION_CONFIG, execution.id, &fields)
            .await
            .unwrap();
        Materialize::seal_execution_inputs(conn, execution.id, QUEUE_KEY)
            .await
            .unwrap();
        let prepared = Variables::prepare_initial_variables(conn, execution.id, QUEUE_KEY)
            .await
            .unwrap();
        let state = WorkflowExecutionRepository::create_or_get_by_execution_with_conn(
            conn,
            CreateWorkflowExecutionInput {
                execution: execution.id,
                workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
                task_graph: json!({}),
                variables: prepared.variables().clone(),
                status: ExecutionStatus::Running,
            },
        )
        .await
        .unwrap();
        assert!(state.created);
        Secrets::replace_entity_with_conn(
            &mut *conn,
            ENTITY_WORKFLOW_VARIABLES,
            execution.id,
            prepared.secrets(),
        )
        .await
        .unwrap();
        prepared.ensure_fresh_on(conn).await.unwrap();
        let original = |path: &str| {
            let field = prepared
                .secrets()
                .iter()
                .find(|field| field.json_path == path)
                .unwrap();
            let provenance: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().unwrap()).unwrap();
            provenance
                .origins
                .into_iter()
                .find(attune_common::repositories::immutable_input_origin::is_input_receipt)
                .unwrap()
        };
        let origin = original("/keyed");
        let same_value_other_site = original("/supplied");
        let credential = RuntimeCredentialBinding {
            execution_id: execution.id,
            admission_id: record.id,
            sponsor_identity_id: record.authority.owner.sponsor_identity_id,
            action_ref: snapshot.executable.action.r#ref.clone(),
            attempt: record.authority.attempt,
        };
        let authority = Authority::validate_execution_credential(conn, &credential)
            .await
            .unwrap();
        let key_target = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Key(key.id),
            credential.sponsor_identity_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            !authority.allows_target(&key_target, attune_common::rbac::Action::Read),
            "delivered component input does not grant optional Key API access"
        );
        for (origin, path) in [(&origin, "/keyed"), (&same_value_other_site, "/supplied")] {
            let context = Inputs::prepare_source(
                conn,
                execution.id,
                &InputSite {
                    execution: execution.id,
                    kind: InputEntityKind::WorkflowVariables,
                    purpose: InputSitePurpose::Value,
                    path: path.into(),
                },
                origin,
                QUEUE_KEY,
            )
            .await
            .unwrap();
            assert_eq!(context.value(), &json!(value));
        }
        Self {
            execution,
            admission: record.id,
            credential,
            origin,
            same_value_other_site,
        }
    }

    pub(super) async fn accept(
        &self,
        conn: &mut sqlx::PgConnection,
        queue: &attune_common::models::WorkQueue,
        item: &attune_common::models::WorkQueueItem,
        snapshot: &ExecutionExecutableSnapshot,
        value: &str,
    ) -> QueueConfigurationInput {
        let fields = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/message/text".into(),
                value: json!(value),
                source_kind: "provenance".into(),
                source_ref: Some(
                    serde_json::to_string(&SecretProvenance {
                        origins: vec![self.origin.clone()],
                        templates: vec![],
                    })
                    .unwrap(),
                ),
            }],
            QUEUE_KEY,
        )
        .unwrap();
        Secrets::replace_entity_with_conn(&mut *conn, ENTITY_QUEUE_ITEM_PAYLOAD, item.id, &fields)
            .await
            .unwrap();
        let source = Authority::validate_execution_credential(conn, &self.credential)
            .await
            .unwrap();
        assert!(
            Fixed::prepare_receipt_transfer(
                conn,
                &source,
                queue,
                item,
                "/message/text",
                &self.same_value_other_site,
                QUEUE_KEY
            )
            .await
            .is_err(),
            "same value/sponsor at another valid source site is not the submitted field's proof"
        );
        assert!(Fixed::prepare_receipt_transfer(
            conn,
            &source,
            queue,
            item,
            "/message/missing",
            &self.origin,
            QUEUE_KEY
        )
        .await
        .is_err());
        let mut wrong = queue.clone();
        wrong.r#ref.push_str("-other");
        assert!(Fixed::prepare_receipt_transfer(
            conn,
            &source,
            &wrong,
            item,
            "/message/text",
            &self.origin,
            QUEUE_KEY
        )
        .await
        .is_err());
        let capability = Fixed::prepare_receipt_transfer(
            conn,
            &source,
            queue,
            item,
            "/message/text",
            &self.origin,
            QUEUE_KEY,
        )
        .await
        .unwrap();
        let mut revoked = sqlx::Connection::begin(conn).await.unwrap();
        Authority::revoke_admission(&mut revoked, self.admission)
            .await
            .unwrap();
        assert!(Fixed::capture_with_transfers(
            &mut revoked,
            queue,
            item,
            snapshot,
            QUEUE_KEY,
            std::slice::from_ref(&capability)
        )
        .await
        .is_err());
        revoked.rollback().await.unwrap();
        let accepted = Fixed::capture_with_transfers(
            conn,
            queue,
            item,
            snapshot,
            QUEUE_KEY,
            std::slice::from_ref(&capability),
        )
        .await
        .unwrap();
        accepted.ensure_fresh_on(conn).await.unwrap();
        accepted
    }
}
