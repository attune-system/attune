//! Real native constructors + F's repository keeper, not API authorization tests.
//! The fixture captures actual A contexts, never invents receipt IDs/consent.
use super::*;
use attune_common::{
    blob_store::{
        body_from_bytes, sha256, BlobStore, FilesystemBlobStore, ObjectKey, StoredObject,
    },
    config::{CacheAdmissionConfig, CacheRetentionConfig},
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
        WorkflowCacheIterationState, WorkflowTaskWaitTarget,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationCleanupOutcome,
            CacheGenerationRepository, CacheIngestRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CacheTransactionMode,
            CreateCacheGenerationInput, CreateCacheGenerationResult, CreateCacheNamespaceInput,
        },
        declared_input::DeclaredInputRepository,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        file_origin::FileOriginRepository,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
            ReceiptContext,
        },
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        runtime_read::{RuntimeReadOrigin, RuntimeReadOwner, RuntimeReadTarget},
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        schema_contract::{BindingOwner, SchemaContractRepository},
        worker_delivery::WorkerDeliveryRepository,
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        workflow_cache_iteration::{
            CreateWorkflowCacheIterationInput, WorkflowCacheIterationRepository,
        },
        workflow_task_wait::{CreateWorkflowTaskWaitInput, WorkflowTaskWaitRepository},
    },
};
use bytes::Bytes;

#[path = "encryption_compatibility.rs"]
mod encryption_compatibility;

const KEY: &str = "F-native-file-repository-test-owned-encryption-key";

#[path = "file_native_prospective_cases.rs"]
mod prospective_cases;

// Requires A's ResultView/finite-completion chain through 76419175 and H's
// ordinary failure projection. These prerequisites are integrated centrally;
// they are intentionally not copied over this older branch's shared files.
#[path = "file_native_completion_cases.rs"]
mod completion_cases;

// Requires A's distinct ordinary iteration INPUT archive through b0d1cf3e.
#[path = "file_native_iteration_outcome_cases.rs"]
mod iteration_outcome_cases;

#[path = "file_success_metadata_custody_cases.rs"]
pub(crate) mod success_metadata_custody_cases;

pub(super) struct Held {
    pub(super) version: Id,
    pub(super) object: StoredObject,
    pub(super) staging: String,
}

async fn native_file_producer_fixture() -> (TestDatabase, Fixture) {
    let (db, mut fixture) = fixture().await;
    // The shared authority-only fixture has no original schema/acceptance.
    // Do not retrofit its running execution. Admit a real file producer with
    // its complete initialization in the original creation transaction.
    let mut tx = db.begin().await.unwrap();
    let executable = attune_common::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(
        &mut tx, &fixture.policy.policy, &fixture.admission.authority.binding,
    ).await.unwrap();
    let selected = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            &fixture.admission.authority.binding,
            &fixture.admission.authority.selected_refs,
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let producer = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(executable.executable.action.id),
            action_ref: executable.executable.action.r#ref.clone(),
            executor: Some(fixture.human_id),
            permission_set_refs: selected.selected_refs.clone(),
            status: ExecutionStatus::Requested,
            config: None,
            ..Default::default()
        },
        &executable,
    )
    .await
    .unwrap();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(producer.id),
        &selected,
    )
    .await
    .unwrap();
    let schema = SchemaContractRepository::materialize_action_pair(&mut tx, &executable)
        .await
        .unwrap();
    schema.validate().unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, producer.id, &schema)
        .await
        .unwrap();
    let stored = SchemaContractRepository::find(&mut tx, BindingOwner::Execution, producer.id)
        .await
        .unwrap()
        .expect("MissingProducerSchemaBinding");
    assert_eq!(stored, schema);
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        producer.id,
        Some(admission.id),
        &executable,
        Some(json!({})),
        &[],
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
        &mut tx, "execution", producer.id, fixture.human_id,
    ).await.unwrap();
    attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
        &mut tx,
        producer.id,
        KEY,
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        producer.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    fixture.credential = RuntimeCredentialBinding {
        execution_id: producer.id,
        admission_id: admission.id,
        sponsor_identity_id: admission.authority.owner.sponsor_identity_id,
        action_ref: producer.action_ref,
        attempt: admission.authority.attempt,
    };
    fixture.admission = admission;
    (db, fixture)
}

async fn hold(
    db: &TestDatabase,
    fixture: &Fixture,
    receipt: &ReceiptContext,
    directory: &std::path::Path,
    store: &FilesystemBlobStore,
) -> Held {
    hold_for_producer(
        db,
        &FileProducer {
            credential: &fixture.credential,
            human_id: fixture.human_id,
            pack_ref: &fixture.policy.policy.owner.pack_ref,
            key: KEY,
        },
        receipt,
        directory,
        store,
    )
    .await
}

struct FileProducer<'a> {
    credential: &'a RuntimeCredentialBinding,
    human_id: Id,
    pack_ref: &'a str,
    key: &'a str,
}

async fn hold_for_producer(
    db: &sqlx::PgPool,
    producer: &FileProducer<'_>,
    receipt: &ReceiptContext,
    directory: &std::path::Path,
    store: &FilesystemBlobStore,
) -> Held {
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, producer.credential)
            .await
            .unwrap();
    let receipt = Inputs::receipt_context(&mut tx, receipt.site(), &receipt.origin(), producer.key)
        .await
        .unwrap();
    let context =
        match WorkerDeliveryRepository::for_execution(&mut tx, producer.credential.execution_id)
            .await
            .unwrap()
        {
            Some(context) => context,
            None => {
                let worker = WorkerRepository::create(
                    &mut *tx,
                    CreateWorkerInput {
                        name: "Native file fixture".into(),
                        worker_type: WorkerType::Local,
                        runtime: None,
                        host: None,
                        port: None,
                        status: Some(WorkerStatus::Active),
                        capabilities: None,
                        meta: None,
                    },
                )
                .await
                .unwrap();
                ExecutionRepository::update(
                    &mut *tx,
                    producer.credential.execution_id,
                    UpdateExecutionInput {
                        worker: Some(worker.id),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
                WorkerDeliveryRepository::register(
                    &mut tx,
                    producer.credential.execution_id,
                    worker.id,
                    uuid::Uuid::new_v4(),
                    Utc::now() + Duration::hours(1),
                )
                .await
                .unwrap()
            }
        };
    let artifact = ArtifactRepository::create(
        &mut *tx,
        CreateArtifactInput {
            r#ref: format!(
                "{}.native_file_{}",
                producer.pack_ref,
                uuid::Uuid::new_v4().simple()
            ),
            scope: OwnerType::Pack,
            owner: producer.pack_ref.to_owned(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: attune_common::models::RetentionPolicyType::Versions,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await
    .unwrap();
    let version = ArtifactVersionRepository::create_object_pending_with_conn(
        &mut tx,
        artifact.id,
        Some(producer.credential.execution_id),
        "text/plain".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let effect = WorkerDeliveryRepository::process_effect(&authority, &context).unwrap();
    WorkerDeliveryRepository::bind(
        &mut tx,
        &context,
        version.id,
        None,
        1048576,
        0,
        Action::Create,
        &effect,
    )
    .await
    .unwrap();
    let staging = format!("native-{}.stage", version.id);
    sqlx::query("UPDATE artifact_version SET file_path=$2 WHERE id=$1")
        .bind(version.id)
        .bind(&staging)
        .execute(&mut *tx)
        .await
        .unwrap();
    // The keeper gets the exact source row from an ACTUAL prepared A context.
    // Do not fabricate C's missing RuntimeReadParents: this test exercises
    // storage custody, not C's separately owned API capture/reader evaluator.
    let source_row = attune_common::repositories::execution_secret_value::SecretProvenanceRow {
        entity_type: receipt.site().kind.as_str().into(),
        entity_id: receipt.site().execution,
        json_path: receipt.site().path.clone(),
        source_kind: "provenance".into(),
        source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
    };
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Artifact(artifact.id),
        producer.human_id,
    )
    .await
    .unwrap()
    .unwrap();
    let read_target: RuntimeReadTarget = serde_json::from_value(json!({
        "kind":"artifact_version", "artifact":artifact.id, "version":version.id,
        "execution":producer.credential.execution_id, "owner":RuntimeReadOwner::of(&target),
    }))
    .unwrap();
    let read = RuntimeReadOrigin {
        target: read_target,
        provenance: vec![],
        parents: vec![],
    };
    FileOriginRepository::reserve(
        &mut tx,
        version.id,
        producer.credential.admission_id,
        producer.credential.execution_id,
        producer.credential.attempt,
        producer.credential.sponsor_identity_id,
        &read,
        &[source_row],
    )
    .await
    .unwrap();
    let root = FileOriginRepository::find(&mut tx, version.id)
        .await
        .unwrap()
        .unwrap()
        .origin_read;
    let (ciphertext, manifest, authenticated) = attune_common::runtime_file_body::seal(
        b"native retained file",
        version.id,
        root,
        producer.key,
    )
    .unwrap();
    FileOriginRepository::complete(&mut tx, &manifest, &authenticated)
        .await
        .unwrap();
    let put = Evidence::begin_put(&mut tx, version.id).await.unwrap();
    receipt.ensure_fresh().unwrap();
    tx.commit().await.unwrap();
    let bytes = Bytes::from(ciphertext);
    tokio::fs::write(directory.join(&staging), &bytes)
        .await
        .unwrap();
    let object = store
        .put_file_atomic(
            &ObjectKey::new(version.object_key.unwrap()).unwrap(),
            body_from_bytes(bytes.clone()),
            bytes.len() as u64,
            sha256(&bytes),
        )
        .await
        .unwrap();
    Evidence::finish_put(db, put, Some(&object)).await.unwrap();
    ArtifactVersionRepository::mark_body_ready(
        db,
        version.id,
        object.provider_version.as_stored(),
        object.size as i64,
        &manifest.ciphertext_sha256,
    )
    .await
    .unwrap()
    .unwrap();
    Held {
        version: version.id,
        object,
        staging,
    }
}

async fn create_generation(
    db: &TestDatabase,
    namespace: Id,
    active: Option<Id>,
    name: &str,
) -> attune_common::models::CacheGeneration {
    match CacheGenerationRepository::create_or_get(
        db.pool(),
        &CreateCacheGenerationInput {
            namespace,
            client_refresh_id: name.into(),
            expected_active_generation: active,
            expected_chunk_count: 1,
            expected_count: None,
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        },
    )
    .await
    .unwrap()
    {
        CreateCacheGenerationResult::Created(g) => g,
        _ => panic!("new owned generation required"),
    }
}

#[tokio::test]
async fn cache_and_wait_archives_pin_files_across_raw_source_gc_then_release_exact_physical_children(
) {
    native_storage_custody_case(false, false).await;
}

#[tokio::test]
async fn encryption_upgrade_preserves_accepted_wait_cache_receipts_and_real_file_bytes() {
    native_storage_custody_case(false, true).await;
}

async fn read_file_payload(
    pool: &sqlx::PgPool,
    held: &Held,
    store: &FilesystemBlobStore,
    key: &str,
) -> Vec<u8> {
    use futures::TryStreamExt;
    let body = FileOriginRepository::body(pool, held.version)
        .await
        .unwrap()
        .unwrap();
    let reader = store
        .get_pinned(
            &held.object.key,
            &held.object.provider_version,
            held.object.size,
            held.object.sha256,
            None,
        )
        .await
        .unwrap();
    let bytes = reader
        .try_fold(Vec::new(), |mut bytes, chunk| async move {
            bytes.extend_from_slice(&chunk);
            Ok(bytes)
        })
        .await
        .unwrap();
    attune_common::runtime_file_body::open(
        &bytes,
        &body.manifest(),
        &body.authenticated_manifest,
        key,
    )
    .unwrap()
}

pub(super) async fn upgrade_held_file(
    pool: &sqlx::PgPool,
    held: &Held,
    store: &FilesystemBlobStore,
    key: &str,
) {
    let before = read_file_payload(pool, held, store, key).await;
    encryption_compatibility::legacy_upgrade_roundtrip(pool, key).await;
    assert_eq!(read_file_payload(pool, held, store, key).await, before);
}

async fn native_storage_custody_case(prospective: bool, compatibility: bool) {
    let (db, fixture) = native_file_producer_fixture().await;
    let owner = &fixture.policy.policy.owner;
    let original_key = helpers::KeyFixture::new_pack(
        owner.pack_id,
        &owner.pack_ref,
        "f_native_original",
        "original wait value",
    )
    .create(&db)
    .await
    .unwrap();
    let input_grant = PermissionSetRepository::create(&db,CreatePermissionSetInput {r#ref:"fixture.f_native_original_input".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original_key.r#ref]}}])}).await.unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: input_grant.id,
        },
    )
    .await
    .unwrap();
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "f_native_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "f_native_leaf")
        .create(&db)
        .await
        .unwrap();
    attune_common::repositories::action::ActionRepository::update(
        &db,
        graph_action.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(json!({"target_execution":{"type":"integer"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    attune_common::repositories::action::ActionRepository::update(
        &db,
        leaf.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(json!({"token":{"type":"string"}})),
            out_schema: Some(json!({"token":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let set_ref = format!("{}.f_native_read", owner.pack_ref);
    let graph_json = json!({"tasks":[
        {"name":"cached","action":leaf.r#ref,"iterate_cache":{"namespace":"f-native","page_size":1},"batch_size":2,"permission_set_refs":["standard"],"params":{"token":"{{ item[0].value }}"}},
        {"name":"observed","action":leaf.r#ref,"wait_for":{"execution":"{{ parameters.target_execution }}"},"permission_set_refs":[set_ref],"params":{"token":"{{ execution.observed.result.token }}"}},
        {"name":"source","action":leaf.r#ref,"permission_set_refs":[]}
    ]});
    let definition = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "F native files".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: graph_json.clone(),
        },
    )
    .await
    .unwrap();
    attune_common::repositories::action::ActionRepository::link_workflow_def(
        &db,
        graph_action.id,
        definition.id,
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![graph_action.id, leaf.id],
        'f',
    )
    .await;
    let executables = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            config: None,
            status: ExecutionStatus::Requested,
            permission_set_refs: vec![],
            ..Default::default()
        },
        &executables[&graph_action.r#ref],
    )
    .await
    .unwrap();
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: definition.id,
            task_graph: graph_json,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let source = ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {action:Some(leaf.id),action_ref:leaf.r#ref.clone(),parent:Some(root.id),
        config:Some(json!({})),result:None,status:ExecutionStatus::Requested,permission_set_refs:vec![],
        workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"source","retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),..Default::default()},&executables[&leaf.r#ref]).await.unwrap();
    let grants = json!([{"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[source.id]}}]);
    let issuer = PermissionSetRepository::create(
        &mut *tx,
        CreatePermissionSetInput {
            r#ref: "fixture.f_native_reader".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: grants.clone(),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &mut *tx,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: issuer.id,
        },
    )
    .await
    .unwrap();
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        owner.pack_id,
        fixture.human_id,
        serde_json::from_value(grants).unwrap(),
        None,
    )
    .await
    .unwrap();
    let set = attune_common::runtime_authority::AdmittedPermissionSet::external(
        &set_ref,
        owner,
        &approval.grants,
        &[approval.approval().unwrap()],
    )
    .unwrap();
    let sets = BTreeMap::from([(set_ref.clone(), set)]);
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let bindings = ["cached", "observed", "source"].map(|name| InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: format!("/tasks/{name}"),
        action_ref: leaf.r#ref.clone(),
    });
    let mut invocations = vec![InvocationPolicy::from_executable(
        root_binding.clone(),
        &executables[&graph_action.r#ref],
        Some(&[]),
        &sets,
    )
    .unwrap()];
    for (binding, refs) in
        bindings
            .iter()
            .zip([vec!["standard".into()], vec![set_ref.clone()], vec![]])
    {
        invocations.push(
            InvocationPolicy::from_executable(
                binding.clone(),
                &executables[&leaf.r#ref],
                Some(&refs),
                &sets,
            )
            .unwrap(),
        );
    }
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
            owner: owner.clone(),
            release_id: release.id,
            release_digest: release.digest,
            concrete_targets: BTreeMap::new(),
            invocations,
        },
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    for (execution, binding, attribution) in [
        (
            root.id,
            &root_binding,
            fixture.admission.authority.attribution.clone(),
        ),
        (
            source.id,
            &bindings[2],
            RequestAttribution::ParentExecution {
                execution_id: root.id,
            },
        ),
    ] {
        let selected = policy
            .policy
            .select(policy.id, binding, &[], attribution)
            .unwrap();
        RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(execution),
            &selected,
        )
        .await
        .unwrap();
    }
    // Initialization requires the full contract bound to this exact pinned
    // invocation. Attach it before the one-shot initializer, not after it.
    for (execution, executable) in [
        (root.id, &executables[&graph_action.r#ref]),
        (source.id, &executables[&leaf.r#ref]),
    ] {
        let schema = SchemaContractRepository::materialize_action_pair(&mut tx, executable)
            .await
            .unwrap();
        let action = &executable.executable.action;
        let empty = json!({});
        assert_eq!(
            schema.input.authored_digest,
            fixture_digest(action.param_schema.as_ref().unwrap_or(&empty))
        );
        assert_eq!(
            schema
                .output
                .as_ref()
                .map(|contract| contract.authored_digest.clone()),
            action.out_schema.as_ref().map(fixture_digest)
        );
        assert_eq!(
            schema.input.source,
            attune_common::value_schema::SourceIdentity::Released {
                component_ref: action.r#ref.clone(),
                release_id: executable.release.id,
                release_digest: executable.release.digest.clone(),
            }
        );
        schema.validate().unwrap();
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution, &schema)
            .await
            .unwrap();
        let stored = SchemaContractRepository::find(&mut tx, BindingOwner::Execution, execution)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored, schema,
            "persist the complete compiled contract, not a digest-only fixture row"
        );
    }
    let root_admission = RuntimeAuthorityRepository::execution_admission(&mut tx, root.id)
        .await
        .unwrap()
        .unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        root.id,
        Some(root_admission.id),
        &executables[&graph_action.r#ref],
        Some(json!({"target_execution":source.id})),
        &[],
    )
    .await
    .unwrap();
    let source_admission = RuntimeAuthorityRepository::execution_admission(&mut tx, source.id)
        .await
        .unwrap()
        .unwrap()
        .id;
    // First input acceptance belongs to admitted nonterminal executions. A
    // completed native wait source must have passed this real producer phase.
    for execution in [root.id, source.id] {
        attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(&mut tx,"execution",execution,fixture.human_id).await.unwrap();
        attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
            &mut tx, execution, KEY,
        )
        .await
        .unwrap();
    }
    for execution in [root.id, source.id] {
        ExecutionRepository::update(
            &mut *tx,
            execution,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Running),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    // This is the source's original output ciphertext, not an input claim.
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/token".into(),
            value: json!("original wait value"),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&attune_common::secret_provenance::SecretProvenance::key(
                    attune_common::key_access::key_origin(&original_key),
                ))
                .unwrap(),
            ),
        }],
        KEY,
    )
    .unwrap();
    attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(&mut tx,attune_common::secret_values::ENTITY_EXECUTION_RESULT,source.id,&fields).await.unwrap();
    ExecutionRepository::update(
        &mut *tx,
        source.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(json!({"token":attune_common::secret_values::redaction_marker()})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let wait = WorkflowTaskWaitRepository::create_or_get(
        &mut tx,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "observed".into(),
            target: WorkflowTaskWaitTarget::Execution(source.id),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        &db,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(owner.pack_id, Some(owner.pack_ref.clone())),
            namespace: "f-native".into(),
            policy: CacheNamespacePolicy::default(),
        },
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let generation = create_generation(&db, namespace.id, None, "first").await;
    let mut cache_entries = vec![
        CacheEntryInput {
            external_id: "a".into(),
            value: json!("cache original"),
            source_updated_at: None,
            source_checksum: Some("original-a".into()),
        },
        CacheEntryInput {
            external_id: "b".into(),
            value: json!("cache second"),
            source_updated_at: None,
            source_checksum: Some("original-b".into()),
        },
    ];
    if prospective {
        cache_entries.push(CacheEntryInput {
            external_id: "c".into(),
            value: json!("cache successor"),
            source_updated_at: None,
            source_checksum: Some("original-c".into()),
        });
    }
    CacheIngestRepository::insert_chunk(&db, generation.id, 0, "f-native-chunk", &cache_entries)
        .await
        .unwrap();
    CacheGenerationRepository::seal(&db, generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        &db,
        namespace.id,
        generation.id,
        None,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut iteration = if prospective {
        None
    } else {
        Some(
            WorkflowCacheIterationRepository::create(
                &mut *tx,
                CreateWorkflowCacheIterationInput {
                    workflow_execution: workflow.id,
                    task_name: "cached".into(),
                    namespace: namespace.id,
                    generation: generation.id,
                    page_size: 1,
                    batch_size: 2,
                    concurrency: 1,
                },
            )
            .await
            .unwrap()
            .id,
        )
    };
    tx.commit().await.unwrap();
    let mut receipts = Vec::new();
    let mut snapshots = Vec::new();
    for (index, binding) in bindings.iter().enumerate().take(2) {
        let mut tx = db.begin().await.unwrap();
        let refs = if index == 0 {
            vec!["standard".into()]
        } else {
            vec![set_ref.clone()]
        };
        let task = Admissions::select(
            &mut tx,
            owner.pack_id,
            binding.clone(),
            &executables[&leaf.r#ref],
            &refs,
            RequestAttribution::ParentExecution {
                execution_id: root.id,
            },
            RuntimeTarget::Component(ComponentTarget::Action, graph_action.id),
        )
        .await
        .unwrap();
        if prospective && index == 0 {
            let (receipt, actual_iteration) =
                prospective_cases::capture_linked_cache(&mut tx, root.id, workflow.id, &task).await;
            iteration = Some(actual_iteration);
            snapshots.push(receipt.native_source_metadata()[0].snapshot());
            receipts.push(receipt);
            tx.commit().await.unwrap();
            continue;
        }
        let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
            .await
            .unwrap();
        let task_path = format!("/workflow/tasks/{index}");
        let control = if index == 1 {
            let selector = declarations
                .iter()
                .find(|d| d.source_path.starts_with(&format!("{task_path}/wait_for/")))
                .unwrap();
            let mut session = Inputs::begin_render(
                &mut tx,
                root.id,
                &selector.declaration_revision,
                &selector.source_path,
                &selector.expression,
                InputSitePurpose::Control,
            )
            .await
            .unwrap();
            Inputs::bind_public_parameter(&mut tx, &mut session, "/target_execution")
                .await
                .unwrap();
            let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
            Some(
                Inputs::record_render(
                    &mut tx,
                    &rendered,
                    &InputSite {
                        execution: root.id,
                        kind: InputEntityKind::WorkflowVariables,
                        purpose: InputSitePurpose::Control,
                        path: selector.source_path.clone(),
                    },
                    rendered.value(),
                    rendered.selections(),
                    KEY,
                )
                .await
                .unwrap(),
            )
        } else {
            None
        };
        let declaration = declarations
            .iter()
            .find(|d| d.source_path.starts_with(&format!("{task_path}/params/")))
            .unwrap();
        let mut session = Inputs::begin_render(
            &mut tx,
            root.id,
            &declaration.declaration_revision,
            &declaration.source_path,
            &declaration.expression,
            InputSitePurpose::Value,
        )
        .await
        .unwrap();
        let prepared_wait;
        let prepared_cache;
        if index == 0 {
            prepared_cache = Some(
                Snapshots::prepare_cache(
                    &mut tx,
                    root.id,
                    &task_path,
                    &task,
                    iteration.unwrap(),
                    65536,
                    KEY,
                )
                .await
                .unwrap(),
            );
            Inputs::bind_cache_input(&mut session, prepared_cache.as_ref().unwrap()).unwrap();
            prepared_wait = None;
        } else {
            prepared_wait = Some(
                Snapshots::prepare_wait(
                    &mut tx,
                    root.id,
                    &task_path,
                    &task,
                    wait.id,
                    control.as_ref(),
                    KEY,
                )
                .await
                .unwrap(),
            );
            Inputs::bind_prepared_wait_input(&mut session, prepared_wait.as_ref().unwrap())
                .unwrap();
            prepared_cache = None;
        }
        let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
        let destination = InputSite {
            execution: root.id,
            kind: InputEntityKind::ExecutionResult,
            purpose: InputSitePurpose::Value,
            path: format!("/native_{index}"),
        };
        let prepared = Inputs::prepare_destination(&mut tx, &rendered, &destination, KEY)
            .await
            .unwrap();
        if let Some(cache) = prepared_cache.as_ref() {
            cache.require_current(&mut tx).await.unwrap();
        }
        let released_wait = if let Some(wait) = prepared_wait {
            Some(Snapshots::release_wait(&mut tx, wait).await.unwrap())
        } else {
            None
        };
        let receipt = Inputs::record_prepared(
            &mut tx,
            &rendered,
            &prepared,
            rendered.value(),
            rendered.selections(),
            KEY,
        )
        .await
        .unwrap();
        if let Some(cache) = prepared_cache.as_ref() {
            cache.ensure_fresh().unwrap();
        }
        if let Some(wait) = released_wait.as_ref() {
            wait.ensure_fresh().unwrap();
        }
        receipt.ensure_fresh().unwrap();
        snapshots.push(receipt.native_source_metadata()[0].snapshot());
        receipts.push(receipt);
        tx.commit().await.unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path().join("objects")).unwrap();
    let mut files = Vec::new();
    for receipt in &receipts {
        files.push(hold(&db, &fixture, receipt, directory.path(), &store).await);
    }
    if compatibility {
        let mut before = Vec::new();
        for file in &files {
            before.push(read_file_payload(&db, file, &store, KEY).await);
        }
        encryption_compatibility::legacy_upgrade_roundtrip(&db, KEY).await;
        for (file, expected) in files.iter().zip(before) {
            assert_eq!(read_file_payload(&db, file, &store, KEY).await, expected);
        }
    }
    let iteration = iteration.unwrap();
    if prospective {
        prospective_cases::assert_pinned_after_raw_iteration_gc(
            &db,
            files[0].version,
            iteration,
            snapshots[0],
        )
        .await;
    }
    // Raw source deletion really succeeds while its archived native proof is
    // owned by a file. No raw execution FK/pin or current lookup substitutes it.
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(ExecutionRepository::delete(&mut *tx, source.id)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    Snapshots::storage_evidence(&mut tx, snapshots[1])
        .await
        .unwrap();
    Inputs::receipt_context(&mut tx, receipts[1].site(), &receipts[1].origin(), KEY)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    RuntimeAuthorityRepository::revoke_admission(&mut tx, source_admission)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    assert!(
        Inputs::receipt_context(&mut tx, receipts[1].site(), &receipts[1].origin(), KEY)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    assert!(
        attune_common::repositories::key::KeyRepository::delete(&db, original_key.id)
            .await
            .unwrap()
    );
    // Keyless keeper/readmission remains positive despite reader revocation.
    sqlx::query("UPDATE file_evidence_owner SET evidence_state='pending_audit' WHERE version=$1")
        .bind(files[1].version)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(Evidence::readmit(db.pool(), files[1].version)
        .await
        .unwrap());
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    for snapshot in &snapshots {
        assert_eq!(
            sqlx::query("DELETE FROM runtime_native_input_snapshot WHERE id=$1")
                .bind(snapshot)
                .execute(&mut *tx)
                .await
                .unwrap()
                .rows_affected(),
            0
        );
    }
    tx.commit().await.unwrap();
    let unowned = create_generation(&db, namespace.id, Some(generation.id), "unowned").await;
    CacheGenerationRepository::fail(&db, unowned.id, "owned fixture failed refresh")
        .await
        .unwrap();
    WorkflowCacheIterationRepository::mark_terminal(
        db.pool(),
        iteration,
        WorkflowCacheIterationState::Completed,
        None,
    )
    .await
    .unwrap();
    CacheNamespaceRepository::tombstone(&db, namespace.id)
        .await
        .unwrap();
    assert!(
        !CacheNamespaceRepository::delete_tombstoned_if_empty(db.pool(), namespace.id)
            .await
            .unwrap(),
        "a file-owned generation must keep the namespace physically present"
    );
    assert_eq!(
        CacheNamespaceRepository::delete_empty_tombstoned_batch(db.pool(), 1)
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE cache_generation SET retired=NOW()-INTERVAL '2 days',readable_until=NOW()-INTERVAL '1 day' WHERE id=$1").bind(generation.id).execute(db.pool()).await.unwrap();
    let candidates = CacheGenerationRepository::select_cleanup_candidates(db.pool(), 1)
        .await
        .unwrap();
    assert_eq!(
        candidates[0].id, unowned.id,
        "oldest physical pin must be excluded BEFORE LIMIT"
    );
    let config = CacheRetentionConfig::default();
    assert_eq!(
        CacheGenerationRepository::drop_if_cleanup_eligible(db.pool(), generation.id, &config)
            .await
            .unwrap(),
        CacheGenerationCleanupOutcome::Ineligible
    );
    assert!(matches!(
        CacheGenerationRepository::drop_if_cleanup_eligible(db.pool(), unowned.id, &config)
            .await
            .unwrap(),
        CacheGenerationCleanupOutcome::Dropped { .. }
    ));
    let entries:Vec<Id>=sqlx::query_scalar("SELECT source FROM file_evidence_pin WHERE owner=$1 AND kind='cache_entry' AND source_generation=$2 ORDER BY source").bind(files[0].version).bind(generation.id).fetch_all(db.pool()).await.unwrap();
    assert_eq!(entries.len(), cache_entries.len());
    let wrong: bool = sqlx::query_scalar("SELECT file_evidence_cache_entry_pinned($1,$2)")
        .bind(unowned.id)
        .bind(entries[0])
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!wrong, "entry identity must include its exact generation");
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    let ids = files.iter().map(|file| file.version).collect::<Vec<_>>();
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &ids)
            .await
            .unwrap()
            .len(),
        2
    );
    for file in &files {
        assert!(ArtifactVersionRepository::delete(&mut *tx, file.version)
            .await
            .unwrap());
    }
    tx.commit().await.unwrap();
    for file in &files {
        store
            .delete(&file.object.key, &file.object.provider_version)
            .await
            .unwrap();
        let mut tx = db.begin().await.unwrap();
        Evidence::lock_reclamation(&mut tx).await.unwrap();
        Evidence::object_removed(&mut tx, file.object.key.as_str(), Some("v:wrong"))
            .await
            .unwrap();
        assert!(
            !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshots[0])
                .await
                .unwrap()
        );
        Evidence::object_removed(
            &mut tx,
            file.object.key.as_str(),
            Some(file.object.provider_version.as_stored()),
        )
        .await
        .unwrap();
        Evidence::staging_removed(&mut tx, "wrong", file.version)
            .await
            .unwrap();
        assert!(
            !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshots[0])
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
    }
    for file in &files {
        tokio::fs::remove_file(directory.path().join(&file.staging))
            .await
            .unwrap();
        let mut tx = db.begin().await.unwrap();
        Evidence::lock_reclamation(&mut tx).await.unwrap();
        Evidence::staging_removed(&mut tx, &file.staging, file.version)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    assert!(matches!(
        CacheGenerationRepository::drop_if_cleanup_eligible(db.pool(), generation.id, &config)
            .await
            .unwrap(),
        CacheGenerationCleanupOutcome::Dropped { .. }
    ));
    assert!(
        CacheNamespaceRepository::delete_tombstoned_if_empty(db.pool(), namespace.id)
            .await
            .unwrap(),
        "exact physical settlement releases generation custody and then its empty namespace"
    );
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(ExecutionRepository::delete(&mut *tx, root.id)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    for _ in 0..16 {
        PackRetentionRepository::collect(
            db.pool(),
            directory.path(),
            Utc::now() - Duration::days(30),
            1,
            1,
        )
        .await
        .unwrap();
    }
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM runtime_native_input_snapshot WHERE id=ANY($1)")
            .bind(&snapshots)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "unowned receipt/snapshot ancestors must progress child-first in bounded passes"
    );
    if prospective {
        prospective_cases::assert_released(&db, iteration).await;
    }
    db.cleanup().await.unwrap();
}

/// Storage custody only. The caller supplies a genuine root receipt and an
/// independently admitted, running writer; this never grants reader authority.
#[expect(
    clippy::too_many_arguments,
    reason = "Keep the fixture's credential, actor, artifact, wait, and physical storage owners explicit"
)]
pub(super) async fn hold_wait_output(
    pool: &sqlx::PgPool,
    credential: &RuntimeCredentialBinding,
    human_id: Id,
    pack_ref: &str,
    key: &str,
    receipt: &ReceiptContext,
    directory: &std::path::Path,
    store: &FilesystemBlobStore,
) -> Held {
    hold_for_producer(
        pool,
        &FileProducer {
            credential,
            human_id,
            pack_ref,
            key,
        },
        receipt,
        directory,
        store,
    )
    .await
}
