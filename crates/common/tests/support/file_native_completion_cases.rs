//! CENTRAL ONLY, UNRUN: actual A ResultView/completion -> F custody after raw GC.
//! Prerequisites: A feb9a8d4 + 76419175, H ordinary failure, current J initialization.
//! No copied A modules, synthetic child snapshot, raw-parent grants or cache/group recipe.
use super::*;
use attune_common::{
    models::OutputFormat,
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::NativeResultViewBinding,
        native_failure_materialization::NativeFailureMaterializationRepository as Failures,
        native_input_snapshot::NativeResultContractKind,
    },
    secret_values::{prepare_secret_values, redact_bound_action_result, SecretSource},
};
use serde_json::Value;

// Direct stderr survives a different test aborting the binary before libtest
// emits captured panic buffers. Fixed stage labels only, never source bytes.
fn stage(label: &'static str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "[F_FINITE_CUSTODY] {label}");
}

#[tokio::test]
async fn ordinary_completion_and_action_data_view_keep_files_after_each_raw_child_is_gone() {
    custody_case(false, false).await;
}

#[tokio::test]
async fn ordinary_archives_require_actual_typed_release_and_original_child_contract() {
    custody_case(true, false).await;
}

#[tokio::test]
async fn encryption_upgrade_preserves_completion_and_result_view_history() {
    custody_case(false, true).await;
}

async fn custody_case(negatives: bool, compatibility: bool) {
    stage(if negatives {
        "negative-case/start"
    } else {
        "positive-case/start"
    });
    let (db, fixture) = native_file_producer_fixture().await;
    let owner = &fixture.policy.policy.owner;
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "f_finite_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "f_finite_leaf")
        .create(&db)
        .await
        .unwrap();
    ActionRepository::update(
        &db,
        leaf.id,
        UpdateActionInput {
            output_format: Some("json".into()),
            out_schema: Some(json!({"token":{"type":"string","secret":true,"required":true}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let definition = json!({"tasks":[
        {"name":"first","action":leaf.r#ref},
        {"name":"second","action":leaf.r#ref}
    ],"output_map":{"answer":"{{ result().token }}"}});
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "F finite custody".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, graph_action.id, graph.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![graph_action.id, leaf.id],
        'd',
    )
    .await;
    let executables = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let bindings = ["first", "second"].map(|name| InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: format!("/tasks/{name}"),
        action_ref: leaf.r#ref.clone(),
    });
    let sets = fixture.policy.policy.invocations[0].permission_sets.clone();
    let refs = sets.keys().cloned().collect::<Vec<_>>();
    let mut invocations = vec![InvocationPolicy::from_executable(
        root_binding.clone(),
        &executables[&graph_action.r#ref],
        Some(&refs),
        &sets,
    )
    .unwrap()];
    for binding in &bindings {
        invocations.push(
            InvocationPolicy::from_executable(
                binding.clone(),
                &executables[&leaf.r#ref],
                Some(&[]),
                &BTreeMap::new(),
            )
            .unwrap(),
        );
    }
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
            owner: owner.clone(),
            release_id: release.id,
            release_digest: release.digest.clone(),
            concrete_targets: fixture.policy.policy.concrete_targets.clone(),
            invocations,
        },
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            executor: Some(fixture.human_id),
            status: ExecutionStatus::Requested,
            config: None,
            permission_set_refs: refs.clone(),
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
            workflow_def: graph.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let mut children = Vec::new();
    for name in ["first", "second"] {
        let child = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(leaf.id),
                action_ref: leaf.r#ref.clone(),
                parent: Some(root.id),
                config: None,
                status: ExecutionStatus::Requested,
                permission_set_refs: vec![],
                workflow_task: Some(
                    serde_json::from_value(
                        json!({"workflow_execution":workflow.id,"task_name":name,
                "retry_count":0,"max_retries":0,"timed_out":false}),
                    )
                    .unwrap(),
                ),
                ..Default::default()
            },
            &executables[&leaf.r#ref],
        )
        .await
        .unwrap();
        children.push(child);
    }
    // Real Requested -> admission -> full schema -> original input capture/seal
    // -> Running. Never initialize an already-terminal original source.
    for index in 0..3 {
        let (execution, binding, executable, selected_refs, attribution) = if index == 0 {
            (
                root.id,
                &root_binding,
                &executables[&graph_action.r#ref],
                refs.clone(),
                fixture.admission.authority.attribution.clone(),
            )
        } else {
            (
                children[index - 1].id,
                &bindings[index - 1],
                &executables[&leaf.r#ref],
                vec![],
                RequestAttribution::ParentExecution {
                    execution_id: root.id,
                },
            )
        };
        let selected = policy
            .policy
            .select(policy.id, binding, &selected_refs, attribution)
            .unwrap();
        let admission = RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(execution),
            &selected,
        )
        .await
        .unwrap();
        let schema = SchemaContractRepository::materialize_action_pair(&mut tx, executable)
            .await
            .unwrap();
        schema.validate().unwrap();
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution, &schema)
            .await
            .unwrap();
        assert_eq!(
            SchemaContractRepository::find(&mut tx, BindingOwner::Execution, execution)
                .await
                .unwrap()
                .unwrap(),
            schema
        );
        ExecutionRepository::initialize_config_with_conn(
            &mut tx,
            execution,
            Some(admission.id),
            executable,
            Some(json!({})),
            &[],
        )
        .await
        .unwrap();
        attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
            &mut tx,"execution",execution,fixture.human_id,
        ).await.unwrap();
        attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
            &mut tx, execution, KEY,
        )
        .await
        .unwrap();
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
    for child in &children {
        // Actual worker envelope pointer != schema-local Local origin pointer.
        // This test freezes encrypted original bytes, never substitutes /token
        // for the real /data/token source row or decrypts in the keyless keeper.
        let schema = SchemaContractRepository::find(&mut tx, BindingOwner::Execution, child.id)
            .await
            .unwrap()
            .expect("FOriginalWorkerOutputMissingSchema");
        schema.validate().unwrap();
        let executable = child
            .executable_snapshot
            .as_ref()
            .expect("FOriginalWorkerOutputMissingExecutable");
        assert_eq!(child.parent, Some(root.id));
        assert_eq!(
            child.workflow_task.as_ref().unwrap().workflow_execution,
            workflow.id
        );
        assert_eq!(
            schema.input.source,
            attune_common::value_schema::SourceIdentity::Released {
                component_ref: executable.executable.action.r#ref.clone(),
                release_id: executable.release.id,
                release_digest: executable.release.digest.clone(),
            }
        );
        assert!(
            schema.output.is_some(),
            "FOriginalWorkerOutputMissingOutputContract"
        );
        assert_eq!(
            executable.executable.action.output_format,
            OutputFormat::Json
        );
        // Use the worker's actual contract-bound redaction. A schema-local
        // /token origin at the /data/token envelope field is provenance, not a
        // legacy parameter_schema row whose ref must equal its stored path.
        let (public,original_fields)=redact_bound_action_result(
            json!({"succeeded":false,"data":{"token":format!("original secret child {}",child.id)}}),
            schema.output.as_ref(),executable.executable.action.output_format,child.id,&[],&[],
        ).unwrap();
        assert_eq!(original_fields.len(), 1);
        assert_eq!(original_fields[0].json_path, "/data/token");
        assert_eq!(original_fields[0].source_kind, "provenance");
        let original_provenance: attune_common::secret_provenance::SecretProvenance =
            serde_json::from_str(original_fields[0].source_ref.as_deref().unwrap()).unwrap();
        assert_eq!(
            original_provenance,
            SecretSource::ParameterSchema {
                path: "/token".into()
            }
            .provenance()
        );
        let fields = prepare_secret_values(original_fields, KEY).unwrap();
        Secrets::upsert_many_with_conn(&mut tx, "execution_result", child.id, &fields)
            .await
            .unwrap();
        ExecutionRepository::update(
            &mut *tx,
            child.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Failed),
                result: Some(public),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let child_admissions:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_authority_execution_fence WHERE execution=ANY($1) ORDER BY execution")
        .bind(children.iter().map(|child|child.id).collect::<Vec<_>>()).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(child_admissions.len(), 2);
    // The final delivered completion may account for itself, not invent an
    // earlier sibling's scheduler advancement. Persist that earlier completion
    // from its ACTUAL admitted outcome before preparing the final all-done view.
    stage("prior-failure/prepare-actual-outcome");
    let prior = Inputs::prepare_outcome(&mut tx, root.id, children[0].id)
        .await
        .expect("FPriorFailureOutcomeUnavailable");
    assert_eq!(prior.execution(), children[0].id);
    assert_eq!(prior.task_name(), "first");
    assert_eq!(prior.value("failed"), Some(json!(true)));
    prior.ensure_fresh_on(&mut tx).await.unwrap();
    let advanced = WorkflowExecutionRepository::update(
        &mut *tx,
        workflow.id,
        attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
            failed_tasks: Some(vec![prior.task_name().to_owned()]),
            ..Default::default()
        },
    )
    .await
    .expect("FPriorFailureWorkflowUnavailable");
    assert_eq!(advanced.failed_tasks, vec![prior.task_name().to_owned()]);
    assert_eq!(advanced.status, ExecutionStatus::Running);
    prior.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    stage("original-worker-publication/committed");
    let mut tx = db.begin().await.unwrap();
    stage("result-view/prepare");
    let view = Inputs::prepare_native_result_view(&mut tx, root.id, children[0].id, KEY)
        .await
        .expect("FOriginalJsonResultView: original admitted child after canonical worker output redaction");
    stage("result-view/mapping");
    let mapping = view
        .mappings()
        .iter()
        .find(|m| m.view_path() == "/token")
        .unwrap();
    assert_eq!(mapping.source_path(), "/data/token");
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let declaration = declarations
        .iter()
        .find(|d| d.source_path == "/workflow/output_map/answer")
        .unwrap();
    stage("result-view/begin-render");
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
    Inputs::bind_native_result_view(&mut session, &view, NativeResultViewBinding::CurrentAndTask)
        .unwrap();
    stage("result-view/evaluate");
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    let site = InputSite {
        execution: root.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/answer".into(),
    };
    stage("result-view/prepare-destination");
    let destination =
        Inputs::prepare_native_value_destination(&mut tx, &view, &rendered, &site, KEY)
            .await
            .unwrap();
    assert!(
        Snapshots::storage_evidence(&mut tx, view.snapshot())
            .await
            .is_err(),
        "an actual staged ResultView without release cannot authenticate custody"
    );
    stage("result-view/finalize");
    view.finalize(&mut tx).await.unwrap();
    stage("result-view/record");
    let view_receipt = Inputs::record_native_value(
        &mut tx,
        &view,
        &rendered,
        &destination,
        rendered.value(),
        KEY,
    )
    .await
    .unwrap();
    let view_snapshot = view.snapshot();
    stage("result-view/storage-evidence");
    let evidence = Snapshots::storage_evidence(&mut tx, view_snapshot)
        .await
        .unwrap();
    assert!(evidence.completion_sources().is_empty());
    assert!(evidence.completion_release().is_none());
    assert!(evidence.wait_release().is_none());
    assert_eq!(
        evidence.result_view_release().unwrap().execution(),
        children[0].id
    );
    if negatives {
        snapshot_negatives(&mut tx, view_snapshot, false).await;
    }

    // H's ordinary all-done token is borrowed, not reconstructed from names or
    // confused with a cache/group aggregate. Both real failed children contribute.
    stage("ordinary-failure/prepare-source");
    let state = Failures::prepare_for_completion(&mut tx, children[1].id)
        .await
        .expect("FFinalFailureFactorySourceError")
        .expect("FFinalFailureFactoryDeclinedAfterPriorAdvancement");
    assert_eq!(state.workflow().failed_tasks, vec!["first".to_owned()]);
    assert_eq!(
        state.failed_tasks(),
        ["first".to_owned(), "second".to_owned()]
    );
    stage("ordinary-failure/prepare-metadata");
    let completion = Inputs::prepare_native_completion_metadata(&mut tx, &state, &[], &[], KEY)
        .await
        .unwrap();
    stage("ordinary-failure/prepare-destination");
    let destination = Inputs::prepare_native_completion_destination(&mut tx, &completion, KEY)
        .await
        .unwrap();
    assert!(
        Snapshots::storage_evidence(&mut tx, completion.snapshot())
            .await
            .is_err(),
        "unreleased completion must not authenticate custody"
    );
    completion
        .require_original_rows(state.parent(), state.workflow())
        .unwrap();
    let failed: Vec<String> = sqlx::query_scalar(
        "UPDATE workflow_execution SET failed_tasks=$2 WHERE id=$1 RETURNING failed_tasks",
    )
    .bind(workflow.id)
    .bind(state.failed_tasks())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    stage("ordinary-failure/finalize-after-original-row-guard");
    completion.finalize(&mut tx, &failed).await.unwrap();
    stage("ordinary-failure/record");
    let completion_receipt = Inputs::record_native_completion_metadata(
        &mut tx,
        &completion,
        &destination,
        &failed,
        completion.value(),
        KEY,
    )
    .await
    .unwrap();
    let completion_snapshot = completion.snapshot();
    stage("ordinary-failure/storage-evidence");
    let evidence = Snapshots::storage_evidence(&mut tx, completion_snapshot)
        .await
        .unwrap();
    assert!(evidence.wait_release().is_none());
    assert!(evidence.result_view_release().is_none());
    assert_eq!(
        evidence.archive_owner(),
        ("native_input_snapshot", completion_snapshot)
    );
    let release = evidence.completion_release().unwrap();
    assert_eq!(release.snapshot(), completion_snapshot);
    assert_eq!(release.execution(), root.id);
    assert_eq!(release.completed_execution(), children[1].id);
    assert!(!release.transaction().is_empty());
    assert_eq!(evidence.completion_sources().len(), 2);
    for child in evidence.completion_sources() {
        assert!(children.iter().any(|actual| actual.id == child.execution()));
        assert!(matches!(
            child.contract_kind(),
            NativeResultContractKind::Action {
                format: OutputFormat::Json
            }
        ));
        assert!(child.original_contract().is_some());
        assert_eq!(child.original_fields()[0].json_path, "/data/token");
        assert!(evidence.physical_parents().iter().any(|parent|matches!(parent,
            attune_common::repositories::native_input_snapshot::NativeStorageParent::Execution{execution} if *execution==child.execution())));
    }
    if negatives {
        snapshot_negatives(&mut tx, completion_snapshot, true).await;
    }
    tx.commit().await.unwrap();

    stage("native-receipts/committed");
    // Raw deletion BEFORE F's initial reservation is the key regression: a raw
    // child row cannot accidentally make an incomplete archived-site hook pass.
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    for child in &children {
        assert!(ExecutionRepository::delete(&mut *tx, child.id)
            .await
            .unwrap());
        assert!(
            Secrets::find_by_entity(&mut *tx, "execution_result", child.id)
                .await
                .unwrap()
                .is_empty(),
            "raw producer ciphertext must actually be gone before file reservation"
        );
    }
    tx.commit().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path().join("objects")).unwrap();
    let mut files = Vec::new();
    stage("raw-child-gc/committed");
    for receipt in [&view_receipt, &completion_receipt] {
        stage("file/hold-after-raw-gc");
        files.push(hold(&db, &fixture, receipt, directory.path(), &store).await);
    }
    stage("file/both-holds-complete");
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
    let expected_children = children.iter().map(|child| child.id).collect::<Vec<_>>();
    let pinned:Vec<Id>=sqlx::query_scalar("SELECT source FROM file_evidence_pin WHERE owner=$1 AND kind='native_identity:execution' AND source=ANY($2) ORDER BY source")
        .bind(files[1].version).bind(&expected_children).fetch_all(db.pool()).await.unwrap();
    assert_eq!(pinned, expected_children);
    let admissions:Vec<Id>=sqlx::query_scalar("SELECT source FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_authority_admission' AND source=ANY($2) ORDER BY source")
        .bind(files[1].version).bind(&child_admissions).fetch_all(db.pool()).await.unwrap();
    let mut expected_admissions = child_admissions.clone();
    expected_admissions.sort_unstable();
    assert_eq!(
        admissions, expected_admissions,
        "retain each actual child admission, never the future reader's authority"
    );
    let archive_count:i64=sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_native_input_snapshot' AND source=$2")
        .bind(files[1].version).bind(completion_snapshot).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        archive_count, 1,
        "one real aggregate archive, no synthetic per-child snapshots"
    );
    let raw_pins:i64=sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1 AND kind='execution' AND source=ANY($2)")
        .bind(files[1].version).bind(&expected_children).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        raw_pins, 0,
        "archive-owned child identities are not raw-row pins"
    );
    let snapshots = [view_snapshot, completion_snapshot];
    for file in &files {
        sqlx::query(
            "UPDATE file_evidence_owner SET evidence_state='pending_audit' WHERE version=$1",
        )
        .bind(file.version)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(Evidence::readmit(db.pool(), file.version).await.unwrap());
    }
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    for snapshot in snapshots {
        assert!(
            !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
                .await
                .unwrap()
        );
        assert_eq!(
            sqlx::query("DELETE FROM runtime_native_input_snapshot WHERE id=$1")
                .bind(snapshot)
                .execute(&mut *tx)
                .await
                .unwrap()
                .rows_affected(),
            0
        );
        assert_eq!(sqlx::query("DELETE FROM execution_secret_value WHERE entity_type='native_input_snapshot' AND entity_id=$1")
            .bind(snapshot).execute(&mut *tx).await.unwrap().rows_affected(),0,
            "real archived ciphertext remains physically guarded with its snapshot");
    }
    let versions = files.iter().map(|file| file.version).collect::<Vec<_>>();
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &versions)
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
        Evidence::object_removed(
            &mut tx,
            file.object.key.as_str(),
            Some(file.object.provider_version.as_stored()),
        )
        .await
        .unwrap();
        for snapshot in snapshots {
            assert!(
                !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
                    .await
                    .unwrap()
            );
        }
        tx.commit().await.unwrap();
    }
    // Exact object settlement alone cannot drop shared staging custody.
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
    let pins: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=ANY($1)")
            .bind(&versions)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(pins, 0);
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
            .bind(snapshots.to_vec())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "existing bounded child-first reclamation releases the actual archives"
    );
    let release_count:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM runtime_native_completion_release WHERE snapshot=$1)+(SELECT count(*) FROM runtime_native_result_view_release WHERE snapshot=$2)")
        .bind(completion_snapshot).bind(view_snapshot).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        release_count, 0,
        "typed release rows cascade only with their original archives"
    );
    db.cleanup().await.unwrap();
}

async fn snapshot_negatives(conn: &mut sqlx::PgConnection, snapshot: Id, completion: bool) {
    stage(if completion {
        "negative-copies/completion"
    } else {
        "negative-copies/result-view"
    });
    // Deliberately malformed copies in this isolated negative test only. No raw
    // child/receipt/source histories are synthesized; copy actual archive rows.
    let original: Value =
        sqlx::query_scalar("SELECT body FROM runtime_native_input_snapshot WHERE id=$1")
            .bind(snapshot)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    for corrupt_contract in [false, true] {
        let mut body = original.clone();
        if corrupt_contract {
            if completion {
                body["source"]["children"][0]["contract"] = Value::Null;
            } else {
                body["source"]["contract"] = Value::Null;
            }
        }
        let copy:Id=sqlx::query_scalar("INSERT INTO runtime_native_input_snapshot(renderer_admission,policy_revision,body,body_digest,encrypted_value,encryption_key_hash) SELECT renderer_admission,policy_revision,$2,$3,encrypted_value,encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1 RETURNING id")
            .bind(snapshot).bind(&body).bind(fixture_digest(&body)).fetch_one(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) SELECT $2,receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1")
            .bind(snapshot).bind(copy).execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) SELECT $2,admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1")
            .bind(snapshot).bind(copy).execute(&mut *conn).await.unwrap();
        let fields = Secrets::find_by_entity(&mut *conn, "native_input_snapshot", snapshot)
            .await
            .unwrap();
        // Stored encrypted values are copied without decrypting or changing the
        // original field/provenance/key hash; no re-sealed substitute is used.
        sqlx::query("INSERT INTO execution_secret_value(entity_type,entity_id,json_path,encrypted_value,encryption_key_hash,source_kind,source_ref) SELECT entity_type,$2,json_path,encrypted_value,encryption_key_hash,source_kind,source_ref FROM execution_secret_value WHERE entity_type='native_input_snapshot' AND entity_id=$1")
            .bind(snapshot).bind(copy).execute(&mut *conn).await.unwrap();
        assert!(!fields.is_empty());
        assert!(
            Snapshots::storage_evidence(conn, copy).await.is_err(),
            "a full archive copy cannot replace the actual typed release"
        );
        if completion {
            sqlx::query("INSERT INTO runtime_native_completion_release(snapshot,execution,completed_execution,release_transaction) SELECT $2,execution,completed_execution,release_transaction FROM runtime_native_completion_release WHERE snapshot=$1")
                .bind(snapshot).bind(copy).execute(&mut *conn).await.unwrap();
        } else {
            sqlx::query("INSERT INTO runtime_native_result_view_release(snapshot,source_execution,source_admission,release_transaction) SELECT $2,source_execution,source_admission,release_transaction FROM runtime_native_result_view_release WHERE snapshot=$1")
                .bind(snapshot).bind(copy).execute(&mut *conn).await.unwrap();
        }
        if corrupt_contract {
            assert!(Snapshots::storage_evidence(conn,copy).await.is_err(),"matching release and recomputed outer digest cannot bless a missing original producer contract");
        } else {
            Snapshots::storage_evidence(conn, copy).await.unwrap();
        }
        // Leave the real snapshots unchanged. The test-owned copies have no
        // receipts/files and are removed child-first by normal reclamation.
    }
}
