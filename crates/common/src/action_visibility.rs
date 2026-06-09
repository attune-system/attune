//! Shared policy helpers for action reference visibility.

use crate::{
    error::{Error, Result},
    models::{action::Action, trigger::Trigger, ActionReferenceVisibility},
    workflow::parser::{Task, WorkflowDefinition},
};
use std::collections::BTreeSet;

fn reference_allowed(
    owner_pack_ref: Option<&str>,
    visibility: ActionReferenceVisibility,
    allowed_pack_refs: &[String],
    referencing_pack_ref: Option<&str>,
) -> bool {
    match visibility {
        ActionReferenceVisibility::Public => true,
        ActionReferenceVisibility::Private => {
            referencing_pack_ref.is_some_and(|pack_ref| Some(pack_ref) == owner_pack_ref)
        }
        ActionReferenceVisibility::Restricted => referencing_pack_ref.is_some_and(|pack_ref| {
            Some(pack_ref) == owner_pack_ref
                || allowed_pack_refs.iter().any(|allowed| allowed == pack_ref)
        }),
    }
}

pub fn action_reference_allowed(action: &Action, referencing_pack_ref: Option<&str>) -> bool {
    reference_allowed(
        Some(&action.pack_ref),
        action.reference_visibility,
        &action.reference_allowed_pack_refs,
        referencing_pack_ref,
    )
}

pub fn trigger_reference_allowed(trigger: &Trigger, referencing_pack_ref: Option<&str>) -> bool {
    reference_allowed(
        trigger.pack_ref.as_deref(),
        trigger.reference_visibility,
        &trigger.reference_allowed_pack_refs,
        referencing_pack_ref,
    )
}

pub fn ensure_action_reference_allowed(
    action: &Action,
    referencing_pack_ref: Option<&str>,
    component_kind: &str,
    component_ref: &str,
) -> Result<()> {
    if action_reference_allowed(action, referencing_pack_ref) {
        return Ok(());
    }

    let referencing_pack = referencing_pack_ref.unwrap_or("<no pack>");
    Err(Error::validation(format!(
        "{} '{}' in pack '{}' cannot reference action '{}' because the action is {:?} to pack '{}'",
        component_kind,
        component_ref,
        referencing_pack,
        action.r#ref,
        action.reference_visibility,
        action.pack_ref
    )))
}

pub fn ensure_trigger_reference_allowed(
    trigger: &Trigger,
    referencing_pack_ref: Option<&str>,
    component_kind: &str,
    component_ref: &str,
) -> Result<()> {
    if trigger_reference_allowed(trigger, referencing_pack_ref) {
        return Ok(());
    }

    let referencing_pack = referencing_pack_ref.unwrap_or("<no pack>");
    let owner_pack = trigger.pack_ref.as_deref().unwrap_or("<no pack>");
    Err(Error::validation(format!(
        "{} '{}' in pack '{}' cannot subscribe to trigger '{}' because the trigger is {:?} to pack '{}'",
        component_kind,
        component_ref,
        referencing_pack,
        trigger.r#ref,
        trigger.reference_visibility,
        owner_pack
    )))
}

pub fn collect_workflow_action_refs(workflow: &WorkflowDefinition) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for task in &workflow.tasks {
        collect_task_action_refs(task, &mut refs);
    }
    refs.into_iter().collect()
}

fn collect_task_action_refs(task: &Task, refs: &mut BTreeSet<String>) {
    if let Some(action) = task.action.as_deref() {
        if !action.trim().is_empty() {
            refs.insert(action.trim().to_string());
        }
    }

    if let Some(tasks) = &task.tasks {
        for task in tasks {
            collect_task_action_refs(task, refs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::action::Action;
    use chrono::Utc;
    use serde_json::json;

    fn action(visibility: ActionReferenceVisibility, allowed: Vec<&str>) -> Action {
        Action {
            id: 1,
            r#ref: "owner.do_it".to_string(),
            pack: 1,
            pack_ref: "owner".to_string(),
            label: "Do it".to_string(),
            description: None,
            entrypoint: "do.sh".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            param_schema: None,
            out_schema: None,
            workflow_def: None,
            is_adhoc: false,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: visibility,
            reference_allowed_pack_refs: allowed.into_iter().map(ToOwned::to_owned).collect(),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            timeout_seconds: None,
            parameter_delivery: Default::default(),
            parameter_format: Default::default(),
            output_format: Default::default(),
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    fn trigger(
        visibility: ActionReferenceVisibility,
        owner_pack_ref: Option<&str>,
        allowed: Vec<&str>,
    ) -> Trigger {
        Trigger {
            id: 1,
            r#ref: "owner.happened".to_string(),
            pack: owner_pack_ref.map(|_| 1),
            pack_ref: owner_pack_ref.map(ToOwned::to_owned),
            label: "Happened".to_string(),
            description: None,
            enabled: true,
            param_schema: None,
            out_schema: None,
            webhook_enabled: false,
            webhook_key: None,
            webhook_config: None,
            sensor: None,
            sensor_ref: None,
            is_adhoc: false,
            reference_visibility: visibility,
            reference_allowed_pack_refs: allowed.into_iter().map(ToOwned::to_owned).collect(),
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    #[test]
    fn public_action_allows_any_pack_and_no_pack() {
        let action = action(ActionReferenceVisibility::Public, Vec::new());
        assert!(action_reference_allowed(&action, Some("other")));
        assert!(action_reference_allowed(&action, None));
    }

    #[test]
    fn private_action_allows_only_same_pack() {
        let action = action(ActionReferenceVisibility::Private, Vec::new());
        assert!(action_reference_allowed(&action, Some("owner")));
        assert!(!action_reference_allowed(&action, Some("other")));
        assert!(!action_reference_allowed(&action, None));
    }

    #[test]
    fn restricted_action_allows_same_pack_and_allow_list() {
        let action = action(ActionReferenceVisibility::Restricted, vec!["allowed"]);
        assert!(action_reference_allowed(&action, Some("owner")));
        assert!(action_reference_allowed(&action, Some("allowed")));
        assert!(!action_reference_allowed(&action, Some("other")));
        assert!(!action_reference_allowed(&action, None));
    }

    #[test]
    fn public_trigger_allows_any_pack_and_no_pack() {
        let trigger = trigger(ActionReferenceVisibility::Public, Some("owner"), Vec::new());
        assert!(trigger_reference_allowed(&trigger, Some("other")));
        assert!(trigger_reference_allowed(&trigger, None));
    }

    #[test]
    fn private_trigger_allows_only_same_pack() {
        let trigger = trigger(
            ActionReferenceVisibility::Private,
            Some("owner"),
            Vec::new(),
        );
        assert!(trigger_reference_allowed(&trigger, Some("owner")));
        assert!(!trigger_reference_allowed(&trigger, Some("other")));
        assert!(!trigger_reference_allowed(&trigger, None));
    }

    #[test]
    fn restricted_trigger_allows_same_pack_and_allow_list() {
        let trigger = trigger(
            ActionReferenceVisibility::Restricted,
            Some("owner"),
            vec!["allowed"],
        );
        assert!(trigger_reference_allowed(&trigger, Some("owner")));
        assert!(trigger_reference_allowed(&trigger, Some("allowed")));
        assert!(!trigger_reference_allowed(&trigger, Some("other")));
        assert!(!trigger_reference_allowed(&trigger, None));
    }

    #[test]
    fn collects_nested_workflow_action_refs() {
        let workflow: WorkflowDefinition = serde_json::from_value(json!({
            "ref": "owner.flow",
            "label": "Flow",
            "version": "1.0.0",
            "tasks": [
                {"name": "first", "action": "owner.first"},
                {
                    "name": "parallel",
                    "type": "parallel",
                    "tasks": [
                        {"name": "nested", "action": "other.nested"}
                    ]
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            collect_workflow_action_refs(&workflow),
            vec!["other.nested".to_string(), "owner.first".to_string()]
        );
    }
}
