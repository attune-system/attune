//! Worker placement primitives used by worker registration and executor scheduling.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuralPlacementCompatibility {
    /// At least one possible worker label set satisfies every hard constraint.
    Compatible,
    /// The complete bounded search proved that no worker label set can satisfy the constraints.
    Incompatible,
    /// An input or search limit prevented a proof in either direction.
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuralPlacementBudget {
    pub max_input_items: usize,
    pub max_input_bytes: usize,
    pub max_search_steps: usize,
}

impl Default for StructuralPlacementBudget {
    fn default() -> Self {
        Self {
            max_input_items: 16_384,
            max_input_bytes: 1_048_576,
            max_search_steps: 262_144,
        }
    }
}

pub fn structural_placement_compatibility(
    placements: &[WorkerPlacement],
) -> StructuralPlacementCompatibility {
    structural_placement_compatibility_with_budget(placements, StructuralPlacementBudget::default())
}

pub fn structural_placement_compatibility_with_budget(
    placements: &[WorkerPlacement],
    budget: StructuralPlacementBudget,
) -> StructuralPlacementCompatibility {
    let mut input_items = budget.max_input_items;
    let mut input_bytes = budget.max_input_bytes;
    let mut domains = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut fixed = BTreeMap::<&str, &str>::new();

    for placement in placements {
        if !charge_input(&mut input_items, &mut input_bytes, 1, 0) {
            return StructuralPlacementCompatibility::Indeterminate;
        }
        for (key, value) in &placement.selector {
            if !charge_input(
                &mut input_items,
                &mut input_bytes,
                1,
                key.len().saturating_add(value.len()),
            ) {
                return StructuralPlacementCompatibility::Indeterminate;
            }
            if fixed.insert(key, value).is_some_and(|old| old != value) {
                return StructuralPlacementCompatibility::Incompatible;
            }
        }
        for term in placement
            .affinity
            .required
            .iter()
            .chain(&placement.affinity.anti_affinity)
        {
            if !charge_input(&mut input_items, &mut input_bytes, 1, 0) {
                return StructuralPlacementCompatibility::Indeterminate;
            }
            for (key, value) in &term.match_labels {
                if !charge_input(
                    &mut input_items,
                    &mut input_bytes,
                    1,
                    key.len().saturating_add(value.len()),
                ) {
                    return StructuralPlacementCompatibility::Indeterminate;
                }
                domains.entry(key).or_default().insert(value);
            }
            for expression in &term.match_expressions {
                if !charge_input(&mut input_items, &mut input_bytes, 1, expression.key.len()) {
                    return StructuralPlacementCompatibility::Indeterminate;
                }
                let domain = domains.entry(&expression.key).or_default();
                for value in &expression.values {
                    if !charge_input(&mut input_items, &mut input_bytes, 1, value.len()) {
                        return StructuralPlacementCompatibility::Indeterminate;
                    }
                    domain.insert(value);
                }
            }
        }
    }

    let domains = domains
        .into_iter()
        .filter(|(key, _)| !fixed.contains_key(key))
        .map(|(key, values)| SearchDomain {
            key,
            values: values.into_iter().collect(),
            prefer_present: key_prefers_present(key, placements),
        })
        .collect::<Vec<_>>();
    let mut choices = vec![0; domains.len()];
    let mut search_steps = budget.max_search_steps;

    loop {
        match placements_match_assignment(placements, &fixed, &domains, &choices, &mut search_steps)
        {
            Some(true) => return StructuralPlacementCompatibility::Compatible,
            Some(false) => {}
            None => return StructuralPlacementCompatibility::Indeterminate,
        }

        let mut advanced = false;
        for index in (0..choices.len()).rev() {
            choices[index] += 1;
            if choices[index] < domains[index].choice_count() {
                advanced = true;
                break;
            }
            choices[index] = 0;
        }
        if !advanced {
            return StructuralPlacementCompatibility::Incompatible;
        }
    }
}

fn charge_input(items: &mut usize, bytes: &mut usize, item_cost: usize, byte_cost: usize) -> bool {
    let Some(remaining_items) = items.checked_sub(item_cost) else {
        return false;
    };
    let Some(remaining_bytes) = bytes.checked_sub(byte_cost) else {
        return false;
    };
    *items = remaining_items;
    *bytes = remaining_bytes;
    true
}

#[derive(Clone, Copy)]
enum AssignedLabel<'a> {
    Missing,
    Referenced(&'a str),
    // All present values absent from the input are equivalent to the solver.
    Other,
}

struct SearchDomain<'a> {
    key: &'a str,
    values: Vec<&'a str>,
    prefer_present: bool,
}

impl<'a> SearchDomain<'a> {
    fn choice_count(&self) -> usize {
        self.values.len() + 2
    }

    fn choice(&self, index: usize) -> AssignedLabel<'a> {
        if self.prefer_present {
            if index < self.values.len() {
                AssignedLabel::Referenced(self.values[index])
            } else if index == self.values.len() {
                AssignedLabel::Other
            } else {
                AssignedLabel::Missing
            }
        } else if index == 0 {
            AssignedLabel::Missing
        } else if index <= self.values.len() {
            AssignedLabel::Referenced(self.values[index - 1])
        } else {
            AssignedLabel::Other
        }
    }
}

fn placements_match_assignment<'a>(
    placements: &'a [WorkerPlacement],
    fixed: &BTreeMap<&'a str, &'a str>,
    domains: &[SearchDomain<'a>],
    choices: &[usize],
    steps: &mut usize,
) -> Option<bool> {
    for placement in placements {
        for (key, expected) in &placement.selector {
            take_step(steps)?;
            if !matches!(
                assigned_label(key, fixed, domains, choices),
                AssignedLabel::Referenced(value) if value == expected
            ) {
                return Some(false);
            }
        }

        if !placement.affinity.required.is_empty() {
            let mut required_matches = false;
            for term in &placement.affinity.required {
                take_step(steps)?;
                if selector_term_matches_assignment(term, fixed, domains, choices, steps)? {
                    required_matches = true;
                    break;
                }
            }
            if !required_matches {
                return Some(false);
            }
        }

        for term in &placement.affinity.anti_affinity {
            take_step(steps)?;
            if selector_term_matches_assignment(term, fixed, domains, choices, steps)? {
                return Some(false);
            }
        }
    }
    Some(true)
}

fn selector_term_matches_assignment<'a>(
    term: &'a WorkerSelectorTerm,
    fixed: &BTreeMap<&'a str, &'a str>,
    domains: &[SearchDomain<'a>],
    choices: &[usize],
    steps: &mut usize,
) -> Option<bool> {
    for (key, expected) in &term.match_labels {
        take_step(steps)?;
        if !matches!(
            assigned_label(key, fixed, domains, choices),
            AssignedLabel::Referenced(value) if value == expected
        ) {
            return Some(false);
        }
    }
    for expression in &term.match_expressions {
        take_step(steps)?;
        let assigned = assigned_label(&expression.key, fixed, domains, choices);
        let matches = match expression.operator {
            LabelExpressionOperator::In => match assigned {
                AssignedLabel::Referenced(value) => {
                    values_contain(&expression.values, value, steps)?
                }
                AssignedLabel::Missing | AssignedLabel::Other => false,
            },
            LabelExpressionOperator::NotIn => match assigned {
                AssignedLabel::Referenced(value) => {
                    !values_contain(&expression.values, value, steps)?
                }
                AssignedLabel::Missing | AssignedLabel::Other => true,
            },
            LabelExpressionOperator::Exists => !matches!(assigned, AssignedLabel::Missing),
            LabelExpressionOperator::DoesNotExist => matches!(assigned, AssignedLabel::Missing),
        };
        if !matches {
            return Some(false);
        }
    }
    Some(true)
}

fn values_contain(values: &[String], expected: &str, steps: &mut usize) -> Option<bool> {
    for value in values {
        take_step(steps)?;
        if value == expected {
            return Some(true);
        }
    }
    Some(false)
}

fn assigned_label<'a>(
    key: &str,
    fixed: &BTreeMap<&'a str, &'a str>,
    domains: &[SearchDomain<'a>],
    choices: &[usize],
) -> AssignedLabel<'a> {
    if let Some(value) = fixed.get(key) {
        return AssignedLabel::Referenced(value);
    }
    domains
        .binary_search_by_key(&key, |domain| domain.key)
        .map(|index| domains[index].choice(choices[index]))
        .unwrap_or(AssignedLabel::Missing)
}

fn take_step(steps: &mut usize) -> Option<()> {
    *steps = steps.checked_sub(1)?;
    Some(())
}

fn key_prefers_present(key: &str, placements: &[WorkerPlacement]) -> bool {
    placements.iter().any(|placement| {
        placement.affinity.required.iter().any(|term| {
            term.match_labels.contains_key(key)
                || term.match_expressions.iter().any(|expression| {
                    expression.key == key
                        && matches!(
                            expression.operator,
                            LabelExpressionOperator::In | LabelExpressionOperator::Exists
                        )
                })
        })
    })
}

pub fn parse_rule_sensor_placement(
    selector: &JsonValue,
    tolerations: &JsonValue,
    affinity: &JsonValue,
) -> Result<WorkerPlacement> {
    if !selector.is_object() {
        return Err(Error::validation(
            "sensor_worker_selector must be an object",
        ));
    }
    if !tolerations.is_array() {
        return Err(Error::validation(
            "sensor_worker_tolerations must be an array",
        ));
    }
    if !affinity.is_object() {
        return Err(Error::validation(
            "sensor_worker_affinity must be an object",
        ));
    }
    Ok(WorkerPlacement {
        selector: parse_worker_selector(selector)?,
        tolerations: parse_worker_tolerations(tolerations)?,
        affinity: parse_worker_affinity(affinity)?,
    })
}

pub fn worker_matches_all_placements(
    labels: &BTreeMap<String, String>,
    taints: &[WorkerTaint],
    placements: &[WorkerPlacement],
) -> bool {
    let tolerations = placements
        .iter()
        .flat_map(|placement| placement.tolerations.iter().cloned())
        .collect::<Vec<_>>();
    placements.iter().all(|placement| {
        worker_matches_placement(
            labels,
            taints,
            &placement.selector,
            &tolerations,
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

    #[test]
    fn combined_placements_pool_tolerations() {
        let taints = vec![WorkerTaint {
            key: "gpu".to_string(),
            value: Some("true".to_string()),
            effect: TaintEffect::NoSchedule,
        }];
        let pack = WorkerPlacement::default();
        let action = WorkerPlacement {
            tolerations: vec![WorkerToleration {
                key: "gpu".to_string(),
                operator: TolerationOperator::Exists,
                value: None,
                effect: Some(TaintEffect::NoSchedule),
            }],
            ..Default::default()
        };

        assert!(worker_matches_all_placements(
            &BTreeMap::new(),
            &taints,
            &[pack, action],
        ));
    }

    fn expression_term(
        key: &str,
        operator: LabelExpressionOperator,
        values: &[&str],
    ) -> WorkerSelectorTerm {
        WorkerSelectorTerm {
            match_labels: BTreeMap::new(),
            match_expressions: vec![WorkerLabelExpression {
                key: key.to_string(),
                operator,
                values: values.iter().map(|value| (*value).to_string()).collect(),
            }],
        }
    }

    fn generated_terms() -> Vec<WorkerSelectorTerm> {
        let mut terms = vec![WorkerSelectorTerm::default()];
        for key in ["k", "q"] {
            terms.push(WorkerSelectorTerm {
                match_labels: make_labels(&[(key, "a")]),
                match_expressions: Vec::new(),
            });
            terms.push(expression_term(key, LabelExpressionOperator::In, &["a"]));
            terms.push(expression_term(key, LabelExpressionOperator::NotIn, &["a"]));
            terms.push(expression_term(key, LabelExpressionOperator::Exists, &[]));
            terms.push(expression_term(
                key,
                LabelExpressionOperator::DoesNotExist,
                &[],
            ));
        }
        terms
    }

    fn has_brute_force_witness(placements: &[WorkerPlacement]) -> bool {
        let choices = [None, Some("a"), Some("b"), Some("other")];
        for k in choices {
            for q in choices {
                let mut labels = BTreeMap::new();
                if let Some(value) = k {
                    labels.insert("k".to_string(), value.to_string());
                }
                if let Some(value) = q {
                    labels.insert("q".to_string(), value.to_string());
                }
                if worker_matches_all_placements(&labels, &[], placements) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn structural_solver_agrees_with_exhaustive_small_cases() {
        let terms = generated_terms();
        let selectors = [
            BTreeMap::new(),
            make_labels(&[("k", "a")]),
            make_labels(&[("k", "b")]),
        ];
        let mut required_groups = vec![Vec::new()];
        required_groups.extend(terms.iter().cloned().map(|term| vec![term]));
        for left in &terms {
            for right in &terms {
                required_groups.push(vec![left.clone(), right.clone()]);
            }
        }
        let mut anti_affinity_groups = vec![Vec::new()];
        anti_affinity_groups.extend(terms.iter().cloned().map(|term| vec![term]));

        for selector in selectors {
            for required in &required_groups {
                for anti_affinity in &anti_affinity_groups {
                    let placements = [WorkerPlacement {
                        selector: selector.clone(),
                        affinity: WorkerAffinity {
                            required: required.clone(),
                            anti_affinity: anti_affinity.clone(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }];
                    let expected = if has_brute_force_witness(&placements) {
                        StructuralPlacementCompatibility::Compatible
                    } else {
                        StructuralPlacementCompatibility::Incompatible
                    };
                    assert_eq!(
                        structural_placement_compatibility(&placements),
                        expected,
                        "selector={selector:?} required={required:?} anti={anti_affinity:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn structural_solver_preserves_conjunction_across_placements() {
        let placements = [
            WorkerPlacement {
                selector: make_labels(&[("k", "a")]),
                ..Default::default()
            },
            WorkerPlacement {
                selector: make_labels(&[("k", "b")]),
                ..Default::default()
            },
        ];

        assert_eq!(
            structural_placement_compatibility(&placements),
            StructuralPlacementCompatibility::Incompatible
        );
    }

    #[test]
    fn structural_solver_reports_input_exhaustion_as_indeterminate() {
        let placements = [WorkerPlacement {
            selector: make_labels(&[("k", "a")]),
            ..Default::default()
        }];
        let budget = StructuralPlacementBudget {
            max_input_items: 1,
            ..StructuralPlacementBudget::default()
        };

        assert_eq!(
            structural_placement_compatibility_with_budget(&placements, budget),
            StructuralPlacementCompatibility::Indeterminate
        );
    }

    #[test]
    fn structural_solver_reports_search_exhaustion_as_indeterminate() {
        let term = expression_term("k", LabelExpressionOperator::Exists, &[]);
        let placements = [WorkerPlacement {
            affinity: WorkerAffinity {
                required: vec![term.clone()],
                anti_affinity: vec![term],
                ..Default::default()
            },
            ..Default::default()
        }];
        let budget = StructuralPlacementBudget {
            max_search_steps: 1,
            ..StructuralPlacementBudget::default()
        };

        assert_eq!(
            structural_placement_compatibility_with_budget(&placements, budget),
            StructuralPlacementCompatibility::Indeterminate
        );
    }

    #[test]
    fn structural_solver_handles_many_keys_without_recursive_search() {
        let match_labels = (0..10_000)
            .map(|index| (format!("label_{index:05}"), "value".to_string()))
            .collect();
        let placements = [WorkerPlacement {
            affinity: WorkerAffinity {
                required: vec![WorkerSelectorTerm {
                    match_labels,
                    match_expressions: Vec::new(),
                }],
                ..Default::default()
            },
            ..Default::default()
        }];
        let budget = StructuralPlacementBudget {
            max_input_items: 20_002,
            max_input_bytes: 1_048_576,
            max_search_steps: 32,
        };

        assert_eq!(
            structural_placement_compatibility_with_budget(&placements, budget),
            StructuralPlacementCompatibility::Indeterminate
        );
    }
}
