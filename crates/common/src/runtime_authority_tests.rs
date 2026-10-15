use super::*;
use serde_json::json;

fn owner() -> PackAuthorityOwner {
    PackAuthorityOwner {
        pack_id: 12,
        pack_ref: "example".into(),
        sponsor_identity_id: 90,
    }
}

fn definition(value: serde_json::Value) -> PermissionSetDefinition {
    serde_json::from_value(value).unwrap()
}

fn emitter_definition() -> PermissionSetDefinition {
    definition(json!({
        "ref": "example.emit",
        "grants": [{"resource": "events", "actions": ["create"]}]
    }))
}

fn action_binding() -> InvocationBinding {
    InvocationBinding::Action {
        action_ref: "example.run".into(),
    }
}

fn attribution() -> RequestAttribution {
    RequestAttribution::Human {
        actor: HumanAttribution {
            identity_id: 42,
            login: "deployer@example.test".into(),
        },
    }
}

fn policy() -> RuntimePolicy {
    let set = emitter_definition().compile(&owner(), &[]).unwrap();
    RuntimePolicy {
        owner: owner(),
        release_id: 7,
        release_digest: "a".repeat(64),
        invocations: vec![InvocationPolicy {
            binding: action_binding(),
            target_release_id: 7,
            target_release_digest: "a".repeat(64),
            permission_sets: BTreeMap::from([(set.reference.clone(), set)]),
            standard_access: true,
        }],
    }
}

fn external_grant() -> Grant {
    serde_json::from_value(json!({
        "resource": "events", "actions": ["create"],
        "constraints": {"refs": ["shared.completed"]}
    }))
    .unwrap()
}

fn approval() -> ExternalApproval {
    ExternalApproval {
        id: 66,
        subject_pack_id: owner().pack_id,
        grants: vec![external_grant()],
    }
}

#[test]
fn authored_permission_scope_defaults_to_pack() {
    let definition = emitter_definition();
    assert_eq!(definition.scope, PermissionScope::Pack);
    assert_eq!(definition.grants[0].scope, None);
    assert_eq!(
        serde_json::from_value::<PermissionSetDefinition>(
            serde_json::to_value(&definition).unwrap()
        )
        .unwrap(),
        definition
    );
}

#[test]
fn local_unscoped_event_grants_keep_an_ownership_envelope() {
    let compiled = emitter_definition().compile(&owner(), &[]).unwrap();
    let mut context = AuthorizationContext::new(42);
    context.target_ref = Some("example.completed".into());
    let grant = &compiled.grants[0];
    assert!(grant.allows(
        &BTreeSet::new(),
        Some(12),
        Resource::Events,
        Action::Create,
        &context
    ));
    assert!(!grant.allows(
        &BTreeSet::new(),
        Some(13),
        Resource::Events,
        Action::Create,
        &context
    ));
    assert!(!grant.allows(
        &BTreeSet::new(),
        None,
        Resource::Events,
        Action::Create,
        &context
    ));
    assert!(!grant.allows(
        &BTreeSet::new(),
        Some(12),
        Resource::Events,
        Action::Read,
        &context
    ));
}

#[test]
fn a_ref_prefix_does_not_prove_target_ownership() {
    let compiled = emitter_definition().compile(&owner(), &[]).unwrap();
    let mut context = AuthorizationContext::new(90);
    context.target_ref = Some("example.forged".into());
    context.pack_ref = Some("example".into());
    assert!(!compiled.grants[0].allows(
        &BTreeSet::new(),
        Some(999),
        Resource::Events,
        Action::Create,
        &context
    ));
}

#[test]
fn local_ref_constraints_narrow_the_ownership_envelope() {
    let definition = definition(json!({
        "ref": "example.emit",
        "grants": [{"resource": "events", "actions": ["create"],
            "constraints": {"refs": ["example.completed"]}}]
    }));
    let compiled = definition.compile(&owner(), &[]).unwrap();
    let mut context = AuthorizationContext::new(90);
    context.target_ref = Some("example.completed".into());
    assert!(compiled.grants[0].allows(
        &BTreeSet::new(),
        Some(12),
        Resource::Events,
        Action::Create,
        &context
    ));
    context.target_ref = Some("example.other".into());
    assert!(!compiled.grants[0].allows(
        &BTreeSet::new(),
        Some(12),
        Resource::Events,
        Action::Create,
        &context
    ));
}

#[test]
fn local_sets_cannot_request_system_administration() {
    for resource in [
        "workers",
        "retention",
        "identities",
        "permissions",
        "audit_log",
    ] {
        let definition = definition(json!({
            "ref": "example.admin", "grants": [{"resource": resource, "actions": ["read"]}]
        }));
        assert!(definition.compile(&owner(), &[]).is_err(), "{resource}");
    }
}

#[test]
fn local_sets_cannot_select_another_pack_or_use_its_definition_ref() {
    let mut definition = emitter_definition();
    definition.reference = "shared.emit".into();
    assert!(definition.compile(&owner(), &[]).is_err());
    let definition = definition_from_pack_selector("shared");
    assert!(definition.compile(&owner(), &[]).is_err());
    assert!(definition_from_pack_selector("example")
        .compile(&owner(), &[])
        .is_ok());
}

fn definition_from_pack_selector(reference: &str) -> PermissionSetDefinition {
    definition(json!({
        "ref": "example.emit", "grants": [{"resource": "events", "actions": ["create"],
            "constraints": {"pack_refs": [reference]}}]
    }))
}

#[test]
fn external_scopes_require_a_covering_approval_for_this_pack() {
    let definition = definition(json!({
        "ref": "example.external", "scope": "external",
        "grants": [external_grant()]
    }));
    assert!(definition.compile(&owner(), &[]).is_err());
    let compiled = definition.compile(&owner(), &[approval()]).unwrap();
    assert_eq!(compiled.grants[0].approval_id(), Some(66));
    let mut wrong_subject = approval();
    wrong_subject.subject_pack_id = 13;
    assert!(definition.compile(&owner(), &[wrong_subject]).is_err());
    let mut broad = definition;
    broad.grants[0].constraints = None;
    assert!(broad.compile(&owner(), &[approval()]).is_err());
}

#[test]
fn foreign_permission_refs_require_explicit_approval_and_never_acquire_a_local_envelope() {
    assert!(
        AdmittedPermissionSet::external("shared.events", &owner(), &[external_grant()], &[])
            .is_err()
    );
    let set = AdmittedPermissionSet::external(
        "shared.events",
        &owner(),
        &[external_grant()],
        &[approval()],
    )
    .unwrap();
    assert_eq!(set.reference, "shared.events");
    assert!(matches!(
        set.grants[0],
        AdmittedGrant::External {
            approval_id: 66,
            ..
        }
    ));
    assert!(AdmittedPermissionSet::external(
        "standard",
        &owner(),
        &[external_grant()],
        &[approval()]
    )
    .is_err());
}

#[test]
fn approval_selection_is_independent_of_input_order() {
    let mut earlier = approval();
    earlier.id = 65;
    let first = AdmittedPermissionSet::external(
        "shared.events",
        &owner(),
        &[external_grant()],
        &[approval(), earlier.clone()],
    )
    .unwrap();
    let second = AdmittedPermissionSet::external(
        "shared.events",
        &owner(),
        &[external_grant()],
        &[earlier, approval()],
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.grants[0].approval_id(), Some(65));
}

#[test]
fn external_grants_require_live_approval_validation_at_consumption() {
    let definition = definition(json!({
        "ref": "example.external", "scope": "external", "grants": [external_grant()]
    }));
    let compiled = definition.compile(&owner(), &[approval()]).unwrap();
    let mut context = AuthorizationContext::new(90);
    context.target_ref = Some("shared.completed".into());
    assert!(!compiled.grants[0].allows(
        &BTreeSet::new(),
        Some(13),
        Resource::Events,
        Action::Create,
        &context
    ));
    assert!(compiled.grants[0].allows(
        &BTreeSet::from([66]),
        Some(13),
        Resource::Events,
        Action::Create,
        &context
    ));
}

#[test]
fn mixed_sets_require_approval_only_for_explicit_external_grants() {
    let definition = definition(json!({
        "ref": "example.mixed", "grants": [
            {"resource": "events", "actions": ["create"]},
            {"resource": "events", "actions": ["create"], "scope": "external",
                "constraints": {"refs": ["shared.completed"]}}
        ]
    }));
    assert!(definition.compile(&owner(), &[]).is_err());
    let compiled = definition.compile(&owner(), &[approval()]).unwrap();
    assert_eq!(compiled.grants[0].approval_id(), None);
    assert_eq!(compiled.grants[1].approval_id(), Some(66));
}

#[test]
fn explicit_pack_scope_can_narrow_an_external_set_default() {
    let definition = definition(json!({
        "ref": "example.local", "scope": "external", "grants": [
            {"resource": "events", "actions": ["create"], "scope": "pack"}
        ]
    }));
    assert_eq!(
        definition.compile(&owner(), &[]).unwrap().grants[0].approval_id(),
        None
    );
}

#[test]
fn every_supported_pack_resource_can_be_admitted_locally() {
    for resource in [
        "packs",
        "actions",
        "policies",
        "queues",
        "queue_items",
        "rules",
        "triggers",
        "executions",
        "events",
        "enforcements",
        "inquiries",
        "keys",
        "caches",
        "artifacts",
        "runtimes",
        "dashboards",
    ] {
        let definition = definition(json!({
            "ref": "example.local", "grants": [{"resource": resource, "actions": ["read"]}]
        }));
        assert!(definition.compile(&owner(), &[]).is_ok(), "{resource}");
    }
}

#[test]
fn local_owner_scopes_never_include_system_or_human_resources() {
    for resource in ["keys", "artifacts", "caches"] {
        for owner_type in ["system", "identity"] {
            let definition = definition(json!({
                "ref": "example.local", "grants": [{
                    "resource": resource, "actions": ["read"],
                    "constraints": {"owner_types": [owner_type]}
                }]
            }));
            assert!(definition.compile(&owner(), &[]).is_err());
        }
    }
    let definition = definition(json!({
        "ref": "example.local", "grants": [{"resource": "keys", "actions": ["read"]}]
    }));
    let compiled = definition.compile(&owner(), &[]).unwrap();
    let mut context = AuthorizationContext::new(90);
    for owner_type in [None, Some(OwnerType::Identity), Some(OwnerType::System)] {
        context.owner_type = owner_type;
        assert!(!compiled.grants[0].allows(
            &BTreeSet::new(),
            Some(12),
            Resource::Keys,
            Action::Read,
            &context
        ));
    }
    context.owner_type = Some(OwnerType::Pack);
    assert!(compiled.grants[0].allows(
        &BTreeSet::new(),
        Some(12),
        Resource::Keys,
        Action::Read,
        &context
    ));
}

#[test]
fn human_relative_grants_cannot_be_transferred_to_a_runtime_principal() {
    for constraints in [
        json!({"owner": "self"}),
        json!({"execution_scope": "self"}),
        json!({"execution_scope": "descendants"}),
        json!({"attributes": {"department": "operations"}}),
    ] {
        let definition = definition(json!({
            "ref": "example.relative", "grants": [
                {"resource": "keys", "actions": ["read"], "constraints": constraints}
            ]
        }));
        assert!(definition.compile(&owner(), &[]).is_err());
    }
}

#[test]
fn empty_or_invalid_constraints_and_actions_fail_closed() {
    for constraints in [
        json!({"refs": []}),
        json!({"refs": [" "]}),
        json!({"pack_refs": []}),
        json!({"owner_types": []}),
        json!({"visibility": []}),
        json!({"ids": [42]}),
        json!({"pack_refs": ["example"], "refs": ["example.completed"]}),
    ] {
        let definition = definition(json!({
            "ref": "example.invalid", "grants": [
                {"resource": "events", "actions": ["create"], "constraints": constraints}
            ]
        }));
        assert!(definition.compile(&owner(), &[]).is_err());
    }
    let mut definition = emitter_definition();
    definition.grants[0].actions.clear();
    assert!(definition.compile(&owner(), &[]).is_err());
}

#[test]
fn authored_definition_rejects_unknown_fields_and_scope_modes() {
    for value in [
        json!({"ref": "example.invalid", "scope": "global", "grants": []}),
        json!({"ref": "example.invalid", "grants": [], "unexpected": true}),
        json!({"ref": "example.invalid", "grants": [
            {"resource": "events", "actions": ["create"], "unexpected": true}
        ]}),
    ] {
        assert!(serde_json::from_value::<PermissionSetDefinition>(value).is_err());
    }
}

#[test]
fn external_approval_conversion_does_not_silently_change_local_scope() {
    let local: PermissionGrantDefinition = serde_json::from_value(json!({
        "resource": "events", "actions": ["create"], "scope": "pack"
    }))
    .unwrap();
    assert!(local.into_external_approval_grant().is_err());
    let external: PermissionGrantDefinition = serde_json::from_value(json!({
        "resource": "events", "actions": ["create"], "scope": "external"
    }))
    .unwrap();
    assert!(external.into_external_approval_grant().is_ok());
}

#[test]
fn policy_selection_allows_only_the_specific_invocation() {
    let policy = policy();
    let selected = policy
        .select(
            33,
            &action_binding(),
            &["example.emit".into()],
            attribution(),
        )
        .unwrap();
    assert!(selected.has_api_access());
    assert_eq!(selected.owner.sponsor_identity_id, 90);
    assert_eq!(selected.attribution, attribution());
    selected.validate_against(&policy).unwrap();
    assert!(policy
        .select(
            33,
            &action_binding(),
            &["example.other".into()],
            attribution()
        )
        .is_err());
    let other = InvocationBinding::Action {
        action_ref: "example.other".into(),
    };
    assert!(policy
        .select(33, &other, &["example.emit".into()], attribution())
        .is_err());
}

#[test]
fn empty_selection_is_a_valid_reduction_with_no_api_credential() {
    let selected = policy()
        .select(33, &action_binding(), &[], attribution())
        .unwrap();
    assert!(!selected.has_api_access());
    assert!(selected.grants.is_empty());
    selected.validate_against(&policy()).unwrap();
}

#[test]
fn standard_access_is_explicit_and_not_an_event_grant() {
    let selected = policy()
        .select(33, &action_binding(), &["standard".into()], attribution())
        .unwrap();
    assert!(selected.has_api_access());
    assert!(selected.grants.is_empty());
    let mut policy = policy();
    policy.invocations[0].standard_access = false;
    assert!(policy
        .select(33, &action_binding(), &["standard".into()], attribution())
        .is_err());
}

#[test]
fn selected_refs_are_canonical_and_attempts_are_distinct() {
    let requested = vec![
        "standard".into(),
        "example.emit".into(),
        "example.emit".into(),
    ];
    let first = policy()
        .select(33, &action_binding(), &requested, attribution())
        .unwrap();
    let second = policy()
        .select(33, &action_binding(), &requested, attribution())
        .unwrap();
    assert_eq!(first.selected_refs, vec!["example.emit", "standard"]);
    assert_ne!(first.attempt, second.attempt);
    assert!(!first.attempt.is_nil());
}

#[test]
fn stored_selection_cannot_change_grants_refs_owner_or_attempt() {
    let policy = policy();
    let selected = policy
        .select(
            33,
            &action_binding(),
            &["example.emit".into()],
            attribution(),
        )
        .unwrap();
    let mut altered = selected.clone();
    altered.grants.clear();
    assert!(altered.validate_against(&policy).is_err());
    let mut altered = selected.clone();
    altered.selected_refs.push("example.emit".into());
    assert!(altered.validate_against(&policy).is_err());
    let mut altered = selected.clone();
    altered.owner.pack_id = 13;
    assert!(altered.validate_against(&policy).is_err());
    let mut altered = selected;
    altered.attempt = Uuid::nil();
    assert!(altered.validate_against(&policy).is_err());
}

#[test]
fn mixed_requester_attribution_does_not_change_runtime_selection() {
    let batch = RequestAttribution::QueueBatch {
        queue_ref: "example.jobs".into(),
        items: BTreeMap::from([
            (
                1,
                Some(HumanAttribution {
                    identity_id: 42,
                    login: "one@example.test".into(),
                }),
            ),
            (
                2,
                Some(HumanAttribution {
                    identity_id: 43,
                    login: "two@example.test".into(),
                }),
            ),
            (3, None),
        ]),
    };
    let policy = policy();
    let selected = policy
        .select(
            33,
            &action_binding(),
            &["example.emit".into()],
            batch.clone(),
        )
        .unwrap();
    assert_eq!(selected.attribution, batch);
    selected.validate_against(&policy).unwrap();
    assert_eq!(
        selected.grants,
        policy
            .select(
                33,
                &action_binding(),
                &["example.emit".into()],
                attribution()
            )
            .unwrap()
            .grants
    );
}

#[test]
fn invalid_attribution_fails_before_selection() {
    let empty_batch = RequestAttribution::QueueBatch {
        queue_ref: "example.jobs".into(),
        items: BTreeMap::new(),
    };
    assert!(policy()
        .select(33, &action_binding(), &[], empty_batch)
        .is_err());
    let empty_login = RequestAttribution::Human {
        actor: HumanAttribution {
            identity_id: 42,
            login: "".into(),
        },
    };
    assert!(policy()
        .select(33, &action_binding(), &[], empty_login)
        .is_err());
}

#[test]
fn duplicate_bindings_and_permission_ref_mismatches_are_rejected() {
    let mut duplicate = policy();
    duplicate.invocations.push(duplicate.invocations[0].clone());
    assert!(duplicate.validate().is_err());
    let mut mismatch = policy();
    mismatch.invocations[0]
        .permission_sets
        .get_mut("example.emit")
        .unwrap()
        .reference = "example.other".into();
    assert!(mismatch.validate().is_err());
}

#[test]
fn invalid_policy_owner_release_or_digest_is_rejected() {
    let mut invalid = policy();
    invalid.owner.pack_id = 0;
    assert!(invalid.validate().is_err());
    let mut invalid = policy();
    invalid.owner.sponsor_identity_id = 0;
    assert!(invalid.validate().is_err());
    let mut invalid = policy();
    invalid.release_id = -1;
    assert!(invalid.validate().is_err());
    let mut invalid = policy();
    invalid.release_digest = "A".repeat(64);
    assert!(invalid.validate().is_err());
    assert!(policy()
        .select(0, &action_binding(), &[], attribution())
        .is_err());
}

#[test]
fn forged_local_grant_owner_is_rejected_on_policy_restore() {
    let mut policy = policy();
    policy.invocations[0]
        .permission_sets
        .get_mut("example.emit")
        .unwrap()
        .grants[0] = AdmittedGrant::PackOwned {
        pack_id: 13,
        grant: external_grant(),
    };
    assert!(policy.validate().is_err());
}

#[test]
fn approvals_and_admission_have_no_human_activity_dependency() {
    let approval_value = serde_json::to_value(approval()).unwrap();
    assert!(approval_value.get("issuer_frozen").is_none());
    assert!(approval_value.get("issuer_identity_id").is_none());
    let selected = policy()
        .select(
            33,
            &action_binding(),
            &["example.emit".into()],
            attribution(),
        )
        .unwrap();
    let encoded = serde_json::to_value(&selected).unwrap();
    assert!(encoded.get("requester_active").is_none());
    let restored: SelectedRuntimeAuthority = serde_json::from_value(encoded).unwrap();
    restored.validate_against(&policy()).unwrap();
}

#[test]
fn workflow_rule_and_queue_bindings_remain_distinct() {
    let mut policy = policy();
    let binding = InvocationBinding::WorkflowTask {
        workflow_ref: "example.workflow".into(),
        task_path: "/tasks/0".into(),
        action_ref: "shared.run".into(),
    };
    policy.invocations[0].binding = binding.clone();
    assert!(policy
        .select(33, &binding, &["example.emit".into()], attribution())
        .is_ok());
    for other in [
        InvocationBinding::WorkflowTask {
            workflow_ref: "example.workflow".into(),
            task_path: "/tasks/1".into(),
            action_ref: "shared.run".into(),
        },
        InvocationBinding::Rule {
            rule_ref: "example.rule".into(),
            action_ref: "shared.run".into(),
        },
        InvocationBinding::Queue {
            queue_ref: "example.jobs".into(),
            action_ref: "shared.run".into(),
        },
    ] {
        assert!(policy
            .select(33, &other, &["example.emit".into()], attribution())
            .is_err());
    }
}
