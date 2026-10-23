mod helpers;

use attune_common::{
    models::{EnforcementCondition, EnforcementStatus, ExecutionStatus, OwnerType},
    rbac::Action,
    repositories::{
        component_lifecycle::PackProjectionIds,
        event::{CreateEnforcementInput, EnforcementRepository},
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{CreatePermissionSetInput, PermissionSetRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        rule::{CreateRuleInput, RuleRepository},
        rule_authority::RuleAuthorityRepository,
        Create, FindById,
    },
    secret_provenance::KeyOrigin,
};
use helpers::{create_test_pool, ActionFixture, IdentityFixture, PackFixture, TriggerFixture};
use serde_json::json;

#[tokio::test]
async fn deployed_rule_input_authority_survives_installer_freeze_and_is_not_execution_api_access() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("rule_authority")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
        .create(&db)
        .await
        .unwrap();
    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&db)
        .await
        .unwrap();
    let installer = IdentityFixture::new_unique("authority_installer")
        .create(&db)
        .await
        .unwrap();
    let set = PermissionSetRepository::create(&db, CreatePermissionSetInput {
        r#ref:"test.rule_inputs".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"keys","actions":["read","decrypt","use"],"constraints":{"refs":["system.rule_input"]}}]),
    }).await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let rule = RuleRepository::create(
        &mut *tx,
        CreateRuleInput {
            r#ref: format!("{}.rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Rule fixture".into(),
            description: None,
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref,
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(vec![]),
            enabled: true,
            is_adhoc: false,
            owner_identity: Some(installer.id),
        },
    )
    .await
    .unwrap();
    RuleAuthorityRepository::set_input_refs(&mut tx, rule.id, std::slice::from_ref(&set.r#ref))
        .await
        .unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest: "a".repeat(64),
            object_key: "packs/blobs/authority-fixture.tar.gz".into(),
            provider_version: "e:fixture".into(),
            content_path: "/packs/.releases/authority-fixture".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            triggers: vec![trigger.id],
            rules: vec![rule.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id)
        .await
        .unwrap();
    let rule = RuleRepository::find_by_id(&mut *tx, rule.id)
        .await
        .unwrap()
        .unwrap();
    let snapshot = RuleAuthorityRepository::current(&mut tx, &rule)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(snapshot.identity_id, installer.id);
    assert_eq!(
        snapshot.input_grants[0].actions,
        [Action::Read, Action::Decrypt]
    );
    assert!(snapshot.execution_permission_set_refs.is_empty());
    let metadata_only = attune_common::delegation::DelegationAuthority::new(
        installer.id,
        Default::default(),
        serde_json::from_value(json!([
            {"resource":"rules","actions":["update"]}
        ]))
        .unwrap(),
    );
    assert!(
        RuleAuthorityRepository::require_mutation_authority(&mut tx, &rule, &metadata_only)
            .await
            .is_err()
    );
    let permitted = attune_common::delegation::DelegationAuthority::new(
        installer.id,
        Default::default(),
        snapshot.input_grants.clone(),
    );
    RuleAuthorityRepository::require_mutation_authority(&mut tx, &rule, &permitted)
        .await
        .unwrap();
    sqlx::query("UPDATE identity SET frozen = true WHERE id = $1")
        .bind(installer.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE permission_set SET grants = '[]' WHERE id = $1")
        .bind(set.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let authority = snapshot.input_authority(&mut tx).await.unwrap();
    let origin = KeyOrigin {
        key_id: 7,
        key_ref: "system.rule_input".into(),
        owner_type: OwnerType::System,
        owner_identity: None,
        owner_ref: None,
        encrypted: true,
    };
    assert!(authority.key_allows(&origin, Action::Read));
    assert!(authority.key_allows(&origin, Action::Decrypt));
    assert!(!authority.key_allows(&origin, Action::Use));
    let enforcer = EnforcementRepository::create(
        &mut *tx,
        CreateEnforcementInput {
            rule: Some(rule.id),
            rule_ref: rule.r#ref,
            trigger_ref: rule.trigger_ref,
            config: Some(json!({})),
            event: None,
            status: EnforcementStatus::Created,
            payload: json!({}),
            condition: EnforcementCondition::All,
            conditions: json!({}),
        },
    )
    .await
    .unwrap();
    RuleAuthorityRepository::attach_runtime(&mut tx, "enforcement", enforcer.id, &snapshot)
        .await
        .unwrap();
    let executable = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let input = CreateExecutionInput {
        action: Some(action.id),
        action_ref: action.r#ref,
        config: Some(json!({})),
        env_vars: None,
        parent: None,
        enforcement: Some(enforcer.id),
        executor: Some(snapshot.identity_id),
        permission_set_refs: vec![],
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        worker_selector: None,
        worker_tolerations: None,
        worker_affinity: None,
        worker: None,
        status: ExecutionStatus::Requested,
        trace_tag: None,
        timeout_seconds: Some(60),
        result: None,
        workflow_task: None,
    };
    let execution = ExecutionRepository::create_pinned(&mut *tx, input.clone(), &executable)
        .await
        .unwrap();
    let inherited = RuleAuthorityRepository::runtime(&mut tx, "execution", execution.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        inherited.input_permission_set_refs,
        snapshot.input_permission_set_refs
    );
    let mut child_input = input;
    child_input.parent = Some(execution.id);
    child_input.enforcement = None;
    let child = ExecutionRepository::create_pinned(&mut *tx, child_input, &executable)
        .await
        .unwrap();
    assert_eq!(
        RuleAuthorityRepository::runtime(&mut tx, "execution", child.id)
            .await
            .unwrap()
            .unwrap()
            .identity_id,
        snapshot.identity_id
    );
    let assignments: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM permission_assignment WHERE identity = $1")
            .bind(snapshot.identity_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        assignments, 0,
        "input grants must not become execution API assignments"
    );
    tx.rollback().await.unwrap();
}
