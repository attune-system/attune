//! Worker placement primitives used by worker registration and executor scheduling.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use utoipa::ToSchema;

pub const WORKER_LABELS_CAPABILITY_KEY: &str = "labels";
pub const WORKER_TAINTS_CAPABILITY_KEY: &str = "taints";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaintEffect {
    #[default]
    NoSchedule,
    PreferNoSchedule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerTaint {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default)]
    pub effect: TaintEffect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TolerationOperator {
    #[default]
    Equal,
    Exists,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerToleration {
    pub key: String,
    #[serde(default)]
    pub operator: TolerationOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<TaintEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LabelExpressionOperator {
    In,
    NotIn,
    Exists,
    DoesNotExist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerLabelExpression {
    pub key: String,
    pub operator: LabelExpressionOperator,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerSelectorTerm {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub match_labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_expressions: Vec<WorkerLabelExpression>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PreferredWorkerSelectorTerm {
    #[serde(default = "default_preference_weight")]
    pub weight: i32,
    pub preference: WorkerSelectorTerm,
}

fn default_preference_weight() -> i32 {
    1
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerAffinity {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<WorkerSelectorTerm>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred: Vec<PreferredWorkerSelectorTerm>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anti_affinity: Vec<WorkerSelectorTerm>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerPlacement {
    pub selector: BTreeMap<String, String>,
    pub tolerations: Vec<WorkerToleration>,
    pub affinity: WorkerAffinity,
}

pub fn worker_matches_all_placements(
    labels: &BTreeMap<String, String>,
    taints: &[WorkerTaint],
    placements: &[WorkerPlacement],
) -> bool {
    placements.iter().all(|placement| {
        worker_matches_placement(
            labels,
            taints,
            &placement.selector,
            &placement.tolerations,
            &placement.affinity,
        )
    })
}

pub fn preferred_affinity_score_all(
    labels: &BTreeMap<String, String>,
    placements: &[WorkerPlacement],
) -> i32 {
    placements
        .iter()
        .map(|placement| preferred_affinity_score(labels, &placement.affinity))
        .sum()
}

impl WorkerAffinity {
    pub fn is_empty(&self) -> bool {
        self.required.is_empty() && self.preferred.is_empty() && self.anti_affinity.is_empty()
    }
}

pub fn parse_worker_selector(value: &JsonValue) -> Result<BTreeMap<String, String>> {
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_value(value.clone())
        .map_err(|e| {
            Error::validation(format!(
                "worker_selector must be an object of string labels: {e}"
            ))
        })
        .and_then(|selector: BTreeMap<String, String>| {
            validate_label_map("worker_selector", &selector)?;
            Ok(selector)
        })
}

pub fn parse_worker_tolerations(value: &JsonValue) -> Result<Vec<WorkerToleration>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value::<Vec<WorkerToleration>>(value.clone())
        .map_err(|e| {
            Error::validation(format!(
                "worker_tolerations must be an array of tolerations: {e}"
            ))
        })
        .and_then(|tolerations| {
            validate_tolerations(&tolerations)?;
            Ok(tolerations)
        })
}

pub fn parse_worker_affinity(value: &JsonValue) -> Result<WorkerAffinity> {
    if value.is_null() {
        return Ok(WorkerAffinity::default());
    }
    serde_json::from_value(value.clone())
        .map_err(|e| Error::validation(format!("worker_affinity must be an affinity object: {e}")))
        .and_then(|affinity| {
            validate_affinity(&affinity)?;
            Ok(affinity)
        })
}

pub fn validate_label_map(field_name: &str, labels: &BTreeMap<String, String>) -> Result<()> {
    for (key, value) in labels {
        validate_non_empty_key(field_name, key)?;
        if value.trim().is_empty() {
            return Err(Error::validation(format!(
                "{field_name}['{key}'] must be a non-empty string"
            )));
        }
    }
    Ok(())
}

pub fn validate_taints(taints: &[WorkerTaint]) -> Result<()> {
    for taint in taints {
        validate_non_empty_key("worker taint key", &taint.key)?;
        if matches!(taint.value.as_deref(), Some(value) if value.trim().is_empty()) {
            return Err(Error::validation(format!(
                "worker taint '{}' value must be non-empty when provided",
                taint.key
            )));
        }
    }
    Ok(())
}

pub fn validate_tolerations(tolerations: &[WorkerToleration]) -> Result<()> {
    for toleration in tolerations {
        validate_non_empty_key("worker_tolerations key", &toleration.key)?;
        if toleration.operator == TolerationOperator::Equal
            && matches!(toleration.value.as_deref(), Some(value) if value.trim().is_empty())
        {
            return Err(Error::validation(format!(
                "worker_tolerations '{}' value must be non-empty when provided",
                toleration.key
            )));
        }
    }
    Ok(())
}

pub fn validate_affinity(affinity: &WorkerAffinity) -> Result<()> {
    for term in affinity
        .required
        .iter()
        .chain(affinity.anti_affinity.iter())
        .chain(
            affinity
                .preferred
                .iter()
                .map(|preferred| &preferred.preference),
        )
    {
        validate_selector_term(term)?;
    }

    for preferred in &affinity.preferred {
        if !(1..=100).contains(&preferred.weight) {
            return Err(Error::validation(
                "worker_affinity preferred weights must be between 1 and 100",
            ));
        }
    }

    Ok(())
}

fn validate_selector_term(term: &WorkerSelectorTerm) -> Result<()> {
    validate_label_map("worker_affinity match_labels", &term.match_labels)?;
    for expression in &term.match_expressions {
        validate_non_empty_key("worker_affinity match_expressions key", &expression.key)?;
        match expression.operator {
            LabelExpressionOperator::In | LabelExpressionOperator::NotIn => {
                if expression.values.is_empty() {
                    return Err(Error::validation(format!(
                        "worker_affinity expression '{}' requires at least one value",
                        expression.key
                    )));
                }
                if expression
                    .values
                    .iter()
                    .any(|value| value.trim().is_empty())
                {
                    return Err(Error::validation(format!(
                        "worker_affinity expression '{}' values must be non-empty",
                        expression.key
                    )));
                }
            }
            LabelExpressionOperator::Exists | LabelExpressionOperator::DoesNotExist => {
                if !expression.values.is_empty() {
                    return Err(Error::validation(format!(
                        "worker_affinity expression '{}' must not set values with {:?}",
                        expression.key, expression.operator
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_non_empty_key(field_name: &str, key: &str) -> Result<()> {
    if key.trim().is_empty() {
        return Err(Error::validation(format!("{field_name} must be non-empty")));
    }
    Ok(())
}

pub fn worker_labels_from_capabilities(
    capabilities: Option<&JsonValue>,
) -> BTreeMap<String, String> {
    capabilities
        .and_then(|capabilities| capabilities.get(WORKER_LABELS_CAPABILITY_KEY))
        .and_then(|labels| serde_json::from_value(labels.clone()).ok())
        .unwrap_or_default()
}

pub fn worker_taints_from_capabilities(capabilities: Option<&JsonValue>) -> Vec<WorkerTaint> {
    capabilities
        .and_then(|capabilities| capabilities.get(WORKER_TAINTS_CAPABILITY_KEY))
        .and_then(|taints| serde_json::from_value(taints.clone()).ok())
        .unwrap_or_default()
}

pub fn selector_matches(
    labels: &BTreeMap<String, String>,
    selector: &BTreeMap<String, String>,
) -> bool {
    selector
        .iter()
        .all(|(key, expected)| labels.get(key) == Some(expected))
}

pub fn selector_term_matches(labels: &BTreeMap<String, String>, term: &WorkerSelectorTerm) -> bool {
    selector_matches(labels, &term.match_labels)
        && term
            .match_expressions
            .iter()
            .all(|expression| expression_matches(labels, expression))
}

pub fn worker_matches_placement(
    labels: &BTreeMap<String, String>,
    taints: &[WorkerTaint],
    selector: &BTreeMap<String, String>,
    tolerations: &[WorkerToleration],
    affinity: &WorkerAffinity,
) -> bool {
    selector_matches(labels, selector)
        && taints_tolerated(taints, tolerations)
        && required_affinity_matches(labels, affinity)
        && anti_affinity_allows(labels, affinity)
}

pub fn preferred_affinity_score(
    labels: &BTreeMap<String, String>,
    affinity: &WorkerAffinity,
) -> i32 {
    affinity
        .preferred
        .iter()
        .filter(|preferred| selector_term_matches(labels, &preferred.preference))
        .map(|preferred| preferred.weight)
        .sum()
}

fn required_affinity_matches(labels: &BTreeMap<String, String>, affinity: &WorkerAffinity) -> bool {
    affinity.required.is_empty()
        || affinity
            .required
            .iter()
            .any(|term| selector_term_matches(labels, term))
}

fn anti_affinity_allows(labels: &BTreeMap<String, String>, affinity: &WorkerAffinity) -> bool {
    affinity
        .anti_affinity
        .iter()
        .all(|term| !selector_term_matches(labels, term))
}

fn taints_tolerated(taints: &[WorkerTaint], tolerations: &[WorkerToleration]) -> bool {
    taints.iter().all(|taint| {
        taint.effect != TaintEffect::NoSchedule
            || tolerations
                .iter()
                .any(|toleration| toleration_matches_taint(toleration, taint))
    })
}

fn toleration_matches_taint(toleration: &WorkerToleration, taint: &WorkerTaint) -> bool {
    if toleration.key != taint.key {
        return false;
    }

    if let Some(effect) = toleration.effect {
        if effect != taint.effect {
            return false;
        }
    }

    match toleration.operator {
        TolerationOperator::Exists => true,
        TolerationOperator::Equal => toleration.value.as_deref() == taint.value.as_deref(),
    }
}

fn expression_matches(
    labels: &BTreeMap<String, String>,
    expression: &WorkerLabelExpression,
) -> bool {
    match expression.operator {
        LabelExpressionOperator::In => labels
            .get(&expression.key)
            .is_some_and(|value| expression.values.iter().any(|candidate| candidate == value)),
        LabelExpressionOperator::NotIn => labels
            .get(&expression.key)
            .is_none_or(|value| !expression.values.iter().any(|candidate| candidate == value)),
        LabelExpressionOperator::Exists => labels.contains_key(&expression.key),
        LabelExpressionOperator::DoesNotExist => !labels.contains_key(&expression.key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn selector_and_required_affinity_must_match() {
        let labels = make_labels(&[("gpu", "nvidia"), ("zone", "a")]);
        let selector = make_labels(&[("gpu", "nvidia")]);
        let affinity = WorkerAffinity {
            required: vec![WorkerSelectorTerm {
                match_labels: make_labels(&[("zone", "a")]),
                match_expressions: Vec::new(),
            }],
            ..Default::default()
        };

        assert!(worker_matches_placement(
            &labels,
            &[],
            &selector,
            &[],
            &affinity
        ));
    }

    #[test]
    fn untolerated_no_schedule_taint_rejects_worker() {
        let taints = vec![WorkerTaint {
            key: "gpu".to_string(),
            value: Some("true".to_string()),
            effect: TaintEffect::NoSchedule,
        }];
        let affinity = WorkerAffinity::default();

        assert!(!worker_matches_placement(
            &BTreeMap::new(),
            &taints,
            &BTreeMap::new(),
            &[],
            &affinity
        ));

        let tolerations = vec![WorkerToleration {
            key: "gpu".to_string(),
            operator: TolerationOperator::Equal,
            value: Some("true".to_string()),
            effect: Some(TaintEffect::NoSchedule),
        }];

        assert!(worker_matches_placement(
            &BTreeMap::new(),
            &taints,
            &BTreeMap::new(),
            &tolerations,
            &affinity
        ));
    }

    #[test]
    fn preferred_affinity_scores_matching_terms() {
        let labels = make_labels(&[("zone", "a"), ("disk", "ssd")]);
        let affinity = WorkerAffinity {
            preferred: vec![
                PreferredWorkerSelectorTerm {
                    weight: 50,
                    preference: WorkerSelectorTerm {
                        match_labels: make_labels(&[("zone", "a")]),
                        match_expressions: Vec::new(),
                    },
                },
                PreferredWorkerSelectorTerm {
                    weight: 10,
                    preference: WorkerSelectorTerm {
                        match_labels: make_labels(&[("disk", "hdd")]),
                        match_expressions: Vec::new(),
                    },
                },
            ],
            ..Default::default()
        };

        assert_eq!(preferred_affinity_score(&labels, &affinity), 50);
    }

    #[test]
    fn combined_placements_keep_each_required_affinity_group() {
        let labels = make_labels(&[("zone", "a")]);
        let required_a = WorkerPlacement {
            affinity: WorkerAffinity {
                required: vec![WorkerSelectorTerm {
                    match_labels: make_labels(&[("zone", "a")]),
                    match_expressions: Vec::new(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let required_b = WorkerPlacement {
            affinity: WorkerAffinity {
                required: vec![WorkerSelectorTerm {
                    match_labels: make_labels(&[("gpu", "nvidia")]),
                    match_expressions: Vec::new(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!worker_matches_all_placements(
            &labels,
            &[],
            &[required_a, required_b]
        ));
    }
}
