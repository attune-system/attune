use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::dashboard_data::contracts::{
    default_source_contracts, SourceAvailability, SourceContract, SourceType,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("source `{0}` is unknown")]
    UnknownSourceType(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePlanningStatus {
    Ready,
    Partial,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedSource {
    pub source_type: SourceType,
    pub contract: SourceContract,
    pub planning_status: SourcePlanningStatus,
    pub availability_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SourcePlanner {
    contracts: BTreeMap<SourceType, SourceContract>,
}

impl Default for SourcePlanner {
    fn default() -> Self {
        Self {
            contracts: default_source_contracts(),
        }
    }
}

impl SourcePlanner {
    pub fn contracts(&self) -> &BTreeMap<SourceType, SourceContract> {
        &self.contracts
    }

    pub fn plan_source(&self, source_type: SourceType) -> Result<PlannedSource, PlanError> {
        let contract = self
            .contracts
            .get(&source_type)
            .cloned()
            .ok_or_else(|| PlanError::UnknownSourceType(format!("{source_type:?}")))?;

        let (planning_status, availability_reason) = match contract.availability {
            SourceAvailability::AvailableNow => (SourcePlanningStatus::Ready, None),
            SourceAvailability::Partial => (
                SourcePlanningStatus::Partial,
                contract.notes.map(str::to_string),
            ),
            SourceAvailability::Planned => (
                SourcePlanningStatus::Unsupported,
                Some(
                    contract
                        .notes
                        .unwrap_or("Source is planned but not yet implemented")
                        .to_string(),
                ),
            ),
        };

        Ok(PlannedSource {
            source_type,
            contract,
            planning_status,
            availability_reason,
        })
    }

    pub fn resolve_requested_order(
        &self,
        declaration_order: &[String],
        requested_sources: &[String],
    ) -> Result<Vec<String>, Vec<String>> {
        if requested_sources.is_empty() {
            return Ok(declaration_order.to_vec());
        }

        let declaration_set: BTreeSet<&str> =
            declaration_order.iter().map(String::as_str).collect();
        let unknown: Vec<String> = requested_sources
            .iter()
            .filter(|source_id| !declaration_set.contains(source_id.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(unknown);
        }

        let requested_set: BTreeSet<&str> = requested_sources.iter().map(String::as_str).collect();
        let mut seen = BTreeSet::new();
        let ordered = declaration_order
            .iter()
            .filter(|source_id| requested_set.contains(source_id.as_str()))
            .filter(|source_id| seen.insert((*source_id).clone()))
            .cloned()
            .collect();
        Ok(ordered)
    }
}

#[cfg(test)]
mod tests {
    use crate::dashboard_data::contracts::SourceType;

    use super::{SourcePlanner, SourcePlanningStatus};

    #[test]
    fn available_source_plans_as_ready() {
        let planner = SourcePlanner::default();
        let plan = planner
            .plan_source(SourceType::ExecutionTimeseries)
            .expect("source should exist");
        assert_eq!(plan.planning_status, SourcePlanningStatus::Ready);
        assert!(plan.availability_reason.is_none());
    }

    #[test]
    fn partial_source_plans_as_partial_with_reason() {
        let planner = SourcePlanner::default();
        let plan = planner
            .plan_source(SourceType::EnforcementTimeseries)
            .expect("source should exist");
        assert_eq!(plan.planning_status, SourcePlanningStatus::Partial);
        assert!(plan.availability_reason.is_some());
    }

    #[test]
    fn planned_source_plans_as_unsupported_with_reason() {
        let planner = SourcePlanner::default();
        let plan = planner
            .plan_source(SourceType::QueueThroughput)
            .expect("source should exist");
        assert_eq!(plan.planning_status, SourcePlanningStatus::Unsupported);
        assert!(plan.availability_reason.is_some());
    }

    #[test]
    fn resolve_requested_order_is_deduplicated_and_declaration_stable() {
        let planner = SourcePlanner::default();
        let declaration = vec![
            "event_count".to_string(),
            "queue_backlog".to_string(),
            "worker_health".to_string(),
        ];
        let requested = vec![
            "worker_health".to_string(),
            "queue_backlog".to_string(),
            "worker_health".to_string(),
        ];
        let resolved = planner
            .resolve_requested_order(&declaration, &requested)
            .expect("source ids should be valid");
        assert_eq!(resolved, vec!["queue_backlog", "worker_health"]);
    }

    #[test]
    fn resolve_requested_order_returns_unknown_source_ids() {
        let planner = SourcePlanner::default();
        let declaration = vec!["event_count".to_string(), "queue_backlog".to_string()];
        let requested = vec!["queue_backlog".to_string(), "unknown_source".to_string()];
        let err = planner
            .resolve_requested_order(&declaration, &requested)
            .expect_err("unknown source should fail");
        assert_eq!(err, vec!["unknown_source".to_string()]);
    }
}
