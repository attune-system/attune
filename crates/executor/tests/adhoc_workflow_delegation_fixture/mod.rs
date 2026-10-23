// Independent human-owned ad-hoc characterization. No managed policy admission.
const ADHOC_WORKFLOW_PERMISSION_REF: &str = "fixture.adhoc_workflow_execute";

struct AdhocWorkflowDelegationFixture {
    database: TestDatabase,
    parent: Execution,
    workflow_execution_id: i64,
    graph: TaskGraph,
    context: WorkflowContext,
    identity_id: i64,
    permission_set_id: i64,
    release_files: tempfile::TempDir,
}

impl AdhocWorkflowDelegationFixture {
    async fn create() -> Result<Self> {
        use attune_common::{
            repositories::{
                executable_snapshot::ExecutableSnapshotRepository,
                execution_admission::ExecutionAdmissionRepository as Admission,
                identity::{CreatePermissionSetInput, PermissionSetRepository},
                schema_contract::{BindingOwner, SchemaContractRepository},
                schema_type::SchemaTypeRepository,
            },
            value_schema::CompiledContract,
        };
        use sha2::{Digest, Sha256};

        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let database = TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        let pool = database.pool();
        let release_files = tempfile::tempdir()?;
        let content_path = release_files.path().join("pack");
        std::fs::create_dir_all(&content_path)?;
        let workflow_source = r#"
version: "1.0.0"
tasks:
  - name: restricted_single
    action: adhoc_workflow_delegation_it.consume
    permission_set_refs: "{{ parameters.refs }}"
  - name: restricted_fanout
    action: adhoc_workflow_delegation_it.consume
    permission_set_refs: "{{ item.refs }}"
    with_items: "{{ parameters.items }}"
  - name: permitted_single
    action: adhoc_workflow_delegation_it.consume
    permission_set_refs: "{{ parameters.refs }}"
"#;
        let files = [
            (
                "pack.yaml",
                "ref: adhoc_workflow_delegation_it\nversion: 1.0.0\n",
            ),
            (
                "actions/delegation.yaml",
                "ref: delegation\nworkflow_file: workflows/delegation.workflow.yaml\n",
            ),
            (
                "actions/consume.yaml",
                "ref: consume\nentrypoint: scripts/run.sh\n",
            ),
            (
                "actions/workflows/delegation.workflow.yaml",
                workflow_source,
            ),
            ("scripts/run.sh", "#!/bin/sh\nprintf '{}\\n'\n"),
        ];
        // Own real code bytes and a matching archive digest. No object backend
        // or worker is needed for this repository/scheduler characterization.
        let mut archive = Vec::new();
        for (name, body) in files {
            let destination = content_path.join(name);
            std::fs::create_dir_all(destination.parent().unwrap())?;
            std::fs::write(&destination, body)?;
            #[cfg(unix)]
            if name.ends_with(".sh") {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))?;
            }
            let mut header = [0_u8; 512];
            anyhow::ensure!(name.len() < 100, "fixture tar name is too long");
            header[..name.len()].copy_from_slice(name.as_bytes());
            header[100..108].copy_from_slice(if name.ends_with(".sh") {
                b"0000755\0"
            } else {
                b"0000644\0"
            });
            header[108..116].copy_from_slice(b"0000000\0");
            header[116..124].copy_from_slice(b"0000000\0");
            header[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
            header[136..148].copy_from_slice(b"00000000000\0");
            header[148..156].fill(b' ');
            header[156] = b'0';
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
            header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
            archive.extend_from_slice(&header);
            archive.extend_from_slice(body.as_bytes());
            archive.resize(archive.len() + (512 - body.len() % 512) % 512, 0);
        }
        archive.resize(archive.len() + 1024, 0);
        std::fs::write(release_files.path().join("fixture.tar"), &archive)?;
        let digest = Sha256::digest(&archive)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let pack = PackRepository::create(
            pool,
            CreatePackInput {
                r#ref: "adhoc_workflow_delegation_it".into(),
                label: "Ad-hoc workflow delegation fixture".into(),
                description: None,
                version: "1.0.0".into(),
                conf_schema: serde_json::json!({}),
                config: serde_json::json!({}),
                meta: serde_json::json!({}),
                tags: Vec::new(),
                runtime_deps: Vec::new(),
                dependencies: Vec::new(),
                is_standard: false,
                installers: serde_json::json!({}),
            },
        )
        .await?;
        let identity = IdentityRepository::create(
            pool,
            CreateIdentityInput {
                login: "adhoc-workflow-owner@example.test".into(),
                display_name: None,
                password_hash: None,
                attributes: serde_json::json!({}),
            },
        )
        .await?;
        let mut actions = Vec::new();
        for name in ["delegation", "consume"] {
            actions.push(
                ActionRepository::create(
                    pool,
                    CreateActionInput {
                        r#ref: format!("{}.{}", pack.r#ref, name),
                        pack: pack.id,
                        pack_ref: pack.r#ref.clone(),
                        label: name.into(),
                        description: None,
                        entrypoint: "scripts/run.sh".into(),
                        runtime: None,
                        enabled: true,
                        runtime_version_constraint: None,
                        required_worker_runtimes: serde_json::json!({}),
                        worker_selector: serde_json::json!({}),
                        worker_tolerations: serde_json::json!([]),
                        worker_affinity: serde_json::json!({}),
                        param_schema: None,
                        out_schema: None,
                        is_adhoc: true,
                        accesses_mcp: false,
                        default_execution_permission_set_refs: Vec::new(),
                        reference_visibility: Default::default(),
                        reference_allowed_pack_refs: Vec::new(),
                        log_retention_policy: None,
                        log_retention_limit: None,
                        artifact_retention_policy: None,
                        artifact_retention_limit: None,
                        timeout_seconds: None,
                    },
                )
                .await?,
            );
        }
        let workflow = attune_common::workflow::parse_workflow_yaml(workflow_source)?;
        let graph = TaskGraph::from_workflow(&workflow)?;
        let definition = WorkflowDefinitionRepository::create(
            pool,
            CreateWorkflowDefinitionInput {
                r#ref: actions[0].r#ref.clone(),
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                label: "Ad-hoc workflow delegation".into(),
                description: None,
                version: "1.0.0".into(),
                param_schema: None,
                out_schema: None,
                definition: serde_json::to_value(&workflow)?,
                tags: Vec::new(),
            },
        )
        .await?;
        ActionRepository::link_workflow_def(pool, actions[0].id, definition.id).await?;
        let set = PermissionSetRepository::create(
            pool,
            CreatePermissionSetInput {
                r#ref: ADHOC_WORKFLOW_PERMISSION_REF.into(),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: serde_json::json!([{
                    "resource": "actions",
                    "actions": ["execute"],
                    "constraints": {"refs": [actions[1].r#ref.clone()]}
                }]),
            },
        )
        .await?;
        let mut activation = pool.begin().await?;
        let release = PackReleaseRepository::create_or_get(
            &mut activation,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                version: "1.0.0".into(),
                digest,
                object_key: "adhoc-workflow-delegation-fixture.tar".into(),
                provider_version: "test".into(),
                content_path: content_path.to_string_lossy().into_owned(),
                archive_size: i64::try_from(archive.len())?,
                manifest: serde_json::json!({}),
            },
        )
        .await?;
        // Activate the code release only. Stamping these ad-hoc actions as pack
        // projections or admitting a managed runtime policy would change scope.
        PackReleaseRepository::activate(&mut activation, pack.id, release.id).await?;
        activation.commit().await?;

        let parameters = serde_json::json!({
            "refs": [ADHOC_WORKFLOW_PERMISSION_REF],
            "items": [{"refs": [ADHOC_WORKFLOW_PERMISSION_REF]}]
        });
        let mut acceptance = pool.begin().await?;
        CacheEntryRepository::protect_transaction(
            &mut acceptance,
            CacheTransactionMode::PinMutation,
        )
        .await?;
        SchemaTypeRepository::lock_selection(&mut acceptance).await?;
        let snapshot =
            ExecutableSnapshotRepository::resolve_for_action(&mut *acceptance, actions[0].id)
                .await?;
        anyhow::ensure!(
            snapshot.executable.action.is_adhoc,
            "root must be authored ad-hoc"
        );
        anyhow::ensure!(
            snapshot.pack_executables[&actions[1].r#ref]
                .executable
                .action
                .is_adhoc,
            "consumer must be authored ad-hoc"
        );
        let binding =
            SchemaContractRepository::materialize_action(&mut acceptance, &snapshot).await?;
        let contract = CompiledContract::new(&binding.input)?;
        let values = contract.apply_defaults(&parameters)?;
        contract.validate(&values)?;
        // The owning fixture declares no sensitive schema fields or Key/Pack
        // templates. Supplied refs remain literal JSON, not server declarations.
        let selected_refs = vec!["standard".into()];
        attune_common::delegation::require_execution_refs_for_share(
            &mut acceptance,
            Some(identity.id),
            &selected_refs,
        )
        .await?;
        let provisional = ExecutionRepository::create_pinned(
            &mut *acceptance,
            CreateExecutionInput {
                action: Some(actions[0].id),
                action_ref: actions[0].r#ref.clone(),
                executor: Some(identity.id),
                permission_set_refs: selected_refs,
                status: ExecutionStatus::Requested,
                config: None,
                ..Default::default()
            },
            &snapshot,
        )
        .await?;
        SchemaContractRepository::attach(
            &mut acceptance,
            BindingOwner::Execution,
            provisional.id,
            &binding,
        )
        .await?;
        ExecutionRepository::initialize_config_with_conn(
            &mut acceptance,
            provisional.id,
            None,
            &snapshot,
            Some(values),
            &[],
        )
        .await?;
        // Ad-hoc inputs use the live-owner path. Do not fabricate managed
        // accepted-input seals or recorded human authority without an admission.
        let parent = ExecutionRepository::update(
            &mut *acceptance,
            provisional.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Running),
                ..Default::default()
            },
        )
        .await?;
        Admission::validate_legacy_origin(
            &mut acceptance,
            parent.id,
            identity.id,
            &parent.action_ref,
            &parent.permission_set_refs,
        )
        .await?;
        let state = WorkflowExecutionRepository::create(
            &mut *acceptance,
            CreateWorkflowExecutionInput {
                execution: parent.id,
                workflow_def: definition.id,
                task_graph: serde_json::to_value(&graph)?,
                variables: serde_json::json!({}),
                status: ExecutionStatus::Running,
            },
        )
        .await?;
        acceptance.commit().await?;
        Ok(Self {
            database,
            parent,
            workflow_execution_id: state.id,
            graph,
            context: WorkflowContext::new(parameters, HashMap::new()),
            identity_id: identity.id,
            permission_set_id: set.id,
            release_files,
        })
    }

    async fn cleanup(self) -> Result<()> {
        self.database.cleanup().await?;
        self.release_files.close()?;
        Ok(())
    }
}
