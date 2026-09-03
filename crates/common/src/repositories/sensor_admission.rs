use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgConnection, Row};

use crate::models::{
    Id, Pack, Rule, Runtime, RuntimeVersion, Sensor, Trigger, Worker, WorkerRole, WorkerStatus,
};
use crate::repositories::{
    FindById, PackRepository, RuleRepository, RuntimeRepository, RuntimeVersionRepository,
    SensorRepository, TriggerRepository, WorkerRepository,
};
use crate::runtime_detection::normalize_runtime_name;
use crate::scheduling::{
    parse_rule_sensor_placement, structural_placement_compatibility,
    worker_labels_from_capabilities, worker_matches_all_placements,
    worker_taints_from_capabilities, StructuralPlacementCompatibility, WorkerPlacement,
};
use crate::version_matching::{runtime_version_matches_worker, select_best_version};
use crate::{Error, Result};

const SENSOR_HEARTBEAT_STALE_SECONDS: i64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorAdmissionRequirement {
    Structural,
    Live,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorAdmissionFailureKind {
    PlacementOnUnmanagedTrigger,
    StructuralConflict,
    NoLiveWorker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SensorAdmissionFailure {
    pub sensor_ref: Option<String>,
    pub rule_refs: Vec<String>,
    pub kind: SensorAdmissionFailureKind,
    pub message: String,
}

pub struct SensorAdmissionRepository;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorWorkerEligibility {
    pub active_rule_count: i64,
    pub eligible: bool,
}

impl SensorAdmissionRepository {
    pub async fn lock_mutations(connection: &mut PgConnection) -> Result<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('sensor_rule_admission')::BIGINT)")
            .execute(connection)
            .await?;
        Ok(())
    }

    pub async fn lock_workload_checks(connection: &mut PgConnection) -> Result<()> {
        sqlx::query(
            "SELECT pg_advisory_xact_lock_shared(hashtext('sensor_rule_admission')::BIGINT)",
        )
        .execute(connection)
        .await?;
        Ok(())
    }

    pub async fn assess_rule(
        connection: &mut PgConnection,
        rule_id: Id,
        requirement: SensorAdmissionRequirement,
    ) -> Result<Vec<SensorAdmissionFailure>> {
        let rule = RuleRepository::find_by_id(&mut *connection, rule_id)
            .await?
            .ok_or_else(|| Error::not_found("Rule", "id", rule_id.to_string()))?;
        let Some(trigger_id) = rule.trigger else {
            return Ok(Vec::new());
        };
        let trigger = TriggerRepository::find_by_id(&mut *connection, trigger_id)
            .await?
            .ok_or_else(|| Error::not_found("Trigger", "id", trigger_id.to_string()))?;
        let Some(sensor_id) = trigger.sensor else {
            return unmanaged_trigger_failures(&rule);
        };

        let snapshot = load_sensor_snapshot(connection, sensor_id).await?;
        assess_snapshot(&snapshot, Some(rule.id), requirement)
    }

    pub async fn assess_pack(
        connection: &mut PgConnection,
        pack_id: Id,
        requirement: SensorAdmissionRequirement,
    ) -> Result<Vec<SensorAdmissionFailure>> {
        let rows = sqlx::query(
            "SELECT DISTINCT sensor_id FROM ( \
                 SELECT sensor.id AS sensor_id FROM sensor WHERE sensor.pack = $1 \
                 UNION \
                 SELECT trigger.sensor AS sensor_id \
                 FROM rule JOIN trigger ON trigger.id = rule.trigger \
                 WHERE rule.pack = $1 AND trigger.sensor IS NOT NULL \
             ) affected ORDER BY sensor_id",
        )
        .bind(pack_id)
        .fetch_all(&mut *connection)
        .await?;

        let mut failures = Vec::new();
        for row in rows {
            let sensor_id: Id = row.try_get("sensor_id")?;
            let snapshot = load_sensor_snapshot(connection, sensor_id).await?;
            failures.extend(assess_snapshot(&snapshot, None, requirement)?);
        }

        let unmanaged_rules = sqlx::query_as::<_, Rule>(&format!(
            "SELECT {} FROM rule \
             LEFT JOIN trigger ON trigger.id = rule.trigger \
             WHERE rule.pack = $1 AND trigger.sensor IS NULL ORDER BY rule.ref",
            crate::repositories::rule::SELECT_COLUMNS
                .split(", ")
                .map(|column| format!("rule.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .bind(pack_id)
        .fetch_all(&mut *connection)
        .await?;
        for rule in unmanaged_rules {
            failures.extend(unmanaged_trigger_failures(&rule)?);
        }

        failures.sort_by(|left, right| {
            left.sensor_ref
                .cmp(&right.sensor_ref)
                .then_with(|| left.rule_refs.cmp(&right.rule_refs))
        });
        Ok(failures)
    }

    pub async fn assess_runtime(
        connection: &mut PgConnection,
        runtime_id: Id,
        requirement: SensorAdmissionRequirement,
    ) -> Result<Vec<SensorAdmissionFailure>> {
        let sensor_ids =
            sqlx::query_scalar::<_, Id>("SELECT id FROM sensor WHERE runtime = $1 ORDER BY id")
                .bind(runtime_id)
                .fetch_all(&mut *connection)
                .await?;

        let mut failures = Vec::new();
        for sensor_id in sensor_ids {
            let snapshot = load_sensor_snapshot(connection, sensor_id).await?;
            failures.extend(assess_snapshot(&snapshot, None, requirement)?);
        }
        Ok(failures)
    }

    pub async fn assess_sensor(
        connection: &mut PgConnection,
        sensor_id: Id,
        requirement: SensorAdmissionRequirement,
    ) -> Result<Vec<SensorAdmissionFailure>> {
        let snapshot = load_sensor_snapshot(connection, sensor_id).await?;
        assess_snapshot(&snapshot, None, requirement)
    }

    pub async fn worker_is_eligible(
        connection: &mut PgConnection,
        sensor_id: Id,
        worker_id: Id,
    ) -> Result<bool> {
        let snapshot = load_sensor_snapshot(connection, sensor_id).await?;
        let placements = active_placements(&snapshot)?;
        Ok(snapshot.workers.iter().any(|worker| {
            worker.id == worker_id
                && worker_is_live(worker, snapshot.observed_at)
                && worker_supports_runtime(
                    worker,
                    &snapshot.runtime,
                    &snapshot.runtime_versions,
                    &snapshot.sensor,
                )
                && worker_matches(worker, &placements)
        }))
    }

    pub async fn worker_eligibility_by_sensor(
        connection: &mut PgConnection,
        sensors: &[Sensor],
        worker_id: Id,
    ) -> Result<HashMap<Id, SensorWorkerEligibility>> {
        if sensors.is_empty() {
            return Ok(HashMap::new());
        }

        let observed_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *connection)
            .await?;
        let worker = WorkerRepository::find_by_id(&mut *connection, worker_id).await?;
        let pack_ids = sensors
            .iter()
            .filter_map(|sensor| sensor.pack)
            .collect::<Vec<_>>();
        let packs = sqlx::query_as::<_, Pack>(&format!(
            "SELECT {} FROM pack WHERE id = ANY($1)",
            crate::repositories::pack::PACK_COLUMNS
        ))
        .bind(&pack_ids)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|pack| (pack.id, pack))
        .collect::<HashMap<_, _>>();
        let runtime_ids = sensors
            .iter()
            .map(|sensor| sensor.runtime)
            .collect::<Vec<_>>();
        let runtimes = sqlx::query_as::<_, Runtime>(&format!(
            "SELECT {} FROM runtime WHERE id = ANY($1)",
            crate::repositories::runtime::SELECT_COLUMNS
        ))
        .bind(&runtime_ids)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|runtime| (runtime.id, runtime))
        .collect::<HashMap<_, _>>();
        let mut runtime_versions = HashMap::<Id, Vec<RuntimeVersion>>::new();
        let versions = sqlx::query_as::<_, RuntimeVersion>(&format!(
            "SELECT {} FROM runtime_version WHERE runtime = ANY($1)",
            crate::repositories::runtime_version::SELECT_COLUMNS
        ))
        .bind(&runtime_ids)
        .fetch_all(&mut *connection)
        .await?;
        for version in versions {
            runtime_versions
                .entry(version.runtime)
                .or_default()
                .push(version);
        }

        let sensor_ids = sensors.iter().map(|sensor| sensor.id).collect::<Vec<_>>();
        let active_rules = sqlx::query_as::<_, ActiveRulePlacement>(
            "SELECT trigger.sensor AS sensor_id, \
                    rule.sensor_worker_selector, rule.sensor_worker_tolerations, \
                    rule.sensor_worker_affinity \
             FROM trigger \
             JOIN rule ON rule.trigger = trigger.id \
             WHERE trigger.sensor = ANY($1) \
               AND trigger.enabled = TRUE \
               AND rule.enabled = TRUE",
        )
        .bind(&sensor_ids)
        .fetch_all(&mut *connection)
        .await?;
        let mut active_rules_by_sensor = HashMap::<Id, Vec<ActiveRulePlacement>>::new();
        for rule in active_rules {
            active_rules_by_sensor
                .entry(rule.sensor_id)
                .or_default()
                .push(rule);
        }

        sensors
            .iter()
            .map(|sensor| {
                let runtime = runtimes
                    .get(&sensor.runtime)
                    .ok_or_else(|| Error::not_found("Runtime", "id", sensor.runtime.to_string()))?;
                let active_rules = active_rules_by_sensor
                    .get(&sensor.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let mut placements =
                    sensor_placements(sensor, sensor.pack.and_then(|pack_id| packs.get(&pack_id)));
                for rule in active_rules {
                    placements.push(rule.placement()?);
                }
                let eligible = worker.as_ref().is_some_and(|worker| {
                    worker.worker_role == WorkerRole::Sensor
                        && worker_is_live(worker, observed_at)
                        && worker_supports_runtime(
                            worker,
                            runtime,
                            runtime_versions
                                .get(&runtime.id)
                                .map(Vec::as_slice)
                                .unwrap_or_default(),
                            sensor,
                        )
                        && worker_matches(worker, &placements)
                });
                Ok((
                    sensor.id,
                    SensorWorkerEligibility {
                        active_rule_count: active_rules.len() as i64,
                        eligible,
                    },
                ))
            })
            .collect()
    }

    pub async fn worker_is_eligible_for_active_workload(
        connection: &mut PgConnection,
        sensor_id: Id,
        worker_id: Id,
    ) -> Result<bool> {
        let worker_exists =
            sqlx::query_scalar::<_, bool>("SELECT TRUE FROM worker WHERE id = $1 FOR SHARE")
                .bind(worker_id)
                .fetch_optional(&mut *connection)
                .await?
                .unwrap_or(false);
        if !worker_exists
            || !sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM sensor WHERE id = $1)")
                .bind(sensor_id)
                .fetch_one(&mut *connection)
                .await?
        {
            return Ok(false);
        }

        let snapshot = load_sensor_snapshot(connection, sensor_id).await?;
        let has_active_rule = snapshot
            .trigger_rules
            .iter()
            .any(|(trigger, rule)| trigger.enabled && rule.enabled);
        if !snapshot.sensor.enabled || !has_active_rule {
            return Ok(false);
        }

        let placements = active_placements(&snapshot)?;
        Ok(snapshot.workers.iter().any(|worker| {
            worker.id == worker_id
                && worker_is_live(worker, snapshot.observed_at)
                && worker_supports_runtime(
                    worker,
                    &snapshot.runtime,
                    &snapshot.runtime_versions,
                    &snapshot.sensor,
                )
                && worker_matches(worker, &placements)
        }))
    }
}

#[derive(Debug, FromRow)]
struct ActiveRulePlacement {
    sensor_id: Id,
    sensor_worker_selector: serde_json::Value,
    sensor_worker_tolerations: serde_json::Value,
    sensor_worker_affinity: serde_json::Value,
}

impl ActiveRulePlacement {
    fn placement(&self) -> Result<WorkerPlacement> {
        parse_rule_sensor_placement(
            &self.sensor_worker_selector,
            &self.sensor_worker_tolerations,
            &self.sensor_worker_affinity,
        )
    }
}

struct SensorAdmissionSnapshot {
    observed_at: DateTime<Utc>,
    sensor: Sensor,
    pack: Option<Pack>,
    runtime: Runtime,
    runtime_versions: Vec<RuntimeVersion>,
    trigger_rules: Vec<(Trigger, Rule)>,
    workers: Vec<Worker>,
}

async fn load_sensor_snapshot(
    connection: &mut PgConnection,
    sensor_id: Id,
) -> Result<SensorAdmissionSnapshot> {
    let observed_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *connection)
        .await?;
    let sensor = SensorRepository::find_by_id(&mut *connection, sensor_id)
        .await?
        .ok_or_else(|| Error::not_found("Sensor", "id", sensor_id.to_string()))?;
    let pack = match sensor.pack {
        Some(pack_id) => PackRepository::find_by_id(&mut *connection, pack_id).await?,
        None => None,
    };
    let runtime = RuntimeRepository::find_by_id(&mut *connection, sensor.runtime)
        .await?
        .ok_or_else(|| Error::not_found("Runtime", "id", sensor.runtime.to_string()))?;
    let runtime_versions =
        RuntimeVersionRepository::find_by_runtime(&mut *connection, runtime.id).await?;
    let triggers = TriggerRepository::find_by_sensor(&mut *connection, sensor.id).await?;
    let mut trigger_rules = Vec::new();
    for trigger in triggers {
        for rule in RuleRepository::find_by_trigger(&mut *connection, trigger.id).await? {
            trigger_rules.push((trigger.clone(), rule));
        }
    }
    let workers = WorkerRepository::find_sensor_workers(&mut *connection).await?;
    Ok(SensorAdmissionSnapshot {
        observed_at,
        sensor,
        pack,
        runtime,
        runtime_versions,
        trigger_rules,
        workers,
    })
}

fn assess_snapshot(
    snapshot: &SensorAdmissionSnapshot,
    candidate_rule_id: Option<Id>,
    requirement: SensorAdmissionRequirement,
) -> Result<Vec<SensorAdmissionFailure>> {
    let mut failures = Vec::new();
    let base = base_placements(snapshot);
    let active_rules = snapshot
        .trigger_rules
        .iter()
        .filter(|(trigger, rule)| trigger.enabled && rule.enabled)
        .map(|(_, rule)| rule)
        .collect::<Vec<_>>();

    let active = active_placements(snapshot)?;
    if structural_placement_compatibility(&active) == StructuralPlacementCompatibility::Incompatible
    {
        let mut rule_refs = active_rules
            .iter()
            .map(|rule| rule.r#ref.clone())
            .collect::<Vec<_>>();
        rule_refs.sort();
        failures.push(SensorAdmissionFailure {
            sensor_ref: Some(snapshot.sensor.r#ref.clone()),
            rule_refs,
            kind: SensorAdmissionFailureKind::StructuralConflict,
            message: format!(
                "Enabled rules have conflicting sensor-worker placement for sensor '{}'",
                snapshot.sensor.r#ref
            ),
        });
    }

    for (_, rule) in &snapshot.trigger_rules {
        if rule.enabled || candidate_rule_id.is_some_and(|candidate| candidate != rule.id) {
            continue;
        }
        let mut placements = base.clone();
        for active in &active_rules {
            if active.id != rule.id {
                placements.push(rule_placement(active)?);
            }
        }
        placements.push(rule_placement(rule)?);
        if structural_placement_compatibility(&placements)
            == StructuralPlacementCompatibility::Incompatible
        {
            failures.push(SensorAdmissionFailure {
                sensor_ref: Some(snapshot.sensor.r#ref.clone()),
                rule_refs: vec![rule.r#ref.clone()],
                kind: SensorAdmissionFailureKind::StructuralConflict,
                message: format!(
                    "Rule '{}' has sensor-worker placement that conflicts with its sensor workload",
                    rule.r#ref
                ),
            });
        }
    }

    if snapshot.sensor.enabled
        && !active_rules.is_empty()
        && requirement == SensorAdmissionRequirement::Live
        && failures.is_empty()
    {
        let placements = active_placements(snapshot)?;
        let eligible = snapshot.workers.iter().any(|worker| {
            worker_is_live(worker, snapshot.observed_at)
                && worker_supports_runtime(
                    worker,
                    &snapshot.runtime,
                    &snapshot.runtime_versions,
                    &snapshot.sensor,
                )
                && worker_matches(worker, &placements)
        });
        if !eligible {
            let mut rule_refs = active_rules
                .iter()
                .map(|rule| rule.r#ref.clone())
                .collect::<Vec<_>>();
            rule_refs.sort();
            failures.push(SensorAdmissionFailure {
                sensor_ref: Some(snapshot.sensor.r#ref.clone()),
                rule_refs,
                kind: SensorAdmissionFailureKind::NoLiveWorker,
                message: format!(
                    "No live sensor worker can run the enabled workload for sensor '{}'",
                    snapshot.sensor.r#ref
                ),
            });
        }
    }
    Ok(failures)
}

fn base_placements(snapshot: &SensorAdmissionSnapshot) -> Vec<WorkerPlacement> {
    sensor_placements(&snapshot.sensor, snapshot.pack.as_ref())
}

fn sensor_placements(sensor: &Sensor, pack: Option<&Pack>) -> Vec<WorkerPlacement> {
    let mut placements = Vec::new();
    if let Some(pack) = pack {
        placements.push(WorkerPlacement {
            selector: pack.worker_selector_labels(),
            tolerations: pack.worker_toleration_specs(),
            affinity: pack.worker_affinity_spec(),
        });
    }
    placements.push(WorkerPlacement {
        selector: sensor.worker_selector_labels(),
        tolerations: sensor.worker_toleration_specs(),
        affinity: sensor.worker_affinity_spec(),
    });
    placements
}

fn active_placements(snapshot: &SensorAdmissionSnapshot) -> Result<Vec<WorkerPlacement>> {
    let mut placements = base_placements(snapshot);
    for (trigger, rule) in &snapshot.trigger_rules {
        if trigger.enabled && rule.enabled {
            placements.push(rule_placement(rule)?);
        }
    }
    Ok(placements)
}

fn rule_placement(rule: &Rule) -> Result<WorkerPlacement> {
    parse_rule_sensor_placement(
        &rule.sensor_worker_selector,
        &rule.sensor_worker_tolerations,
        &rule.sensor_worker_affinity,
    )
}

fn unmanaged_trigger_failures(rule: &Rule) -> Result<Vec<SensorAdmissionFailure>> {
    let placement = rule_placement(rule)?;
    if placement.selector.is_empty()
        && placement.tolerations.is_empty()
        && placement.affinity.is_empty()
    {
        return Ok(Vec::new());
    }
    Ok(vec![SensorAdmissionFailure {
        sensor_ref: None,
        rule_refs: vec![rule.r#ref.clone()],
        kind: SensorAdmissionFailureKind::PlacementOnUnmanagedTrigger,
        message: format!(
            "Rule '{}' cannot set sensor-worker placement because its trigger has no managed sensor",
            rule.r#ref
        ),
    }])
}

fn worker_is_live(worker: &Worker, observed_at: DateTime<Utc>) -> bool {
    !worker.cordoned
        && worker.status == Some(WorkerStatus::Active)
        && worker.last_heartbeat.is_some_and(|heartbeat| {
            heartbeat > observed_at - Duration::seconds(SENSOR_HEARTBEAT_STALE_SECONDS)
        })
}

fn worker_matches(worker: &Worker, placements: &[WorkerPlacement]) -> bool {
    worker_matches_all_placements(
        &worker_labels_from_capabilities(worker.capabilities.as_ref()),
        &worker_taints_from_capabilities(worker.capabilities.as_ref()),
        placements,
    )
}

fn worker_supports_runtime(
    worker: &Worker,
    runtime: &Runtime,
    runtime_versions: &[RuntimeVersion],
    sensor: &Sensor,
) -> bool {
    let Some(capabilities) = worker
        .capabilities
        .as_ref()
        .and_then(|value| value.as_object())
    else {
        return false;
    };
    let runtime_names = runtime
        .aliases
        .iter()
        .chain(std::iter::once(&runtime.name))
        .map(|name| normalize_runtime_name(name))
        .collect::<BTreeSet<_>>();
    let advertised_names = capabilities
        .get("runtimes")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(normalize_runtime_name)
        .collect::<BTreeSet<_>>();
    if runtime_names.is_disjoint(&advertised_names) {
        return false;
    }

    let Some(constraint) = sensor.runtime_version_constraint.as_deref() else {
        return true;
    };
    if runtime_versions.is_empty() {
        return false;
    }
    let mut advertised_versions = capabilities
        .get("runtime_versions")
        .and_then(|value| value.as_object())
        .into_iter()
        .flat_map(|versions| {
            runtime_names
                .iter()
                .filter_map(move |name| versions.get(name).and_then(|value| value.as_array()))
        })
        .flatten()
        .filter_map(|value| value.as_str())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if advertised_versions.is_empty() {
        advertised_versions.extend(
            capabilities
                .get("detected_interpreters")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter(|interpreter| {
                    interpreter
                        .get("name")
                        .and_then(|value| value.as_str())
                        .is_some_and(|name| runtime_names.contains(&normalize_runtime_name(name)))
                })
                .filter_map(|interpreter| interpreter.get("version")?.as_str())
                .map(ToOwned::to_owned),
        );
    }
    if advertised_versions.is_empty() {
        return false;
    }
    let local_versions = runtime_versions
        .iter()
        .filter(|version| {
            advertised_versions
                .iter()
                .any(|advertised| runtime_version_matches_worker(version, advertised))
        })
        .cloned()
        .map(|mut version| {
            version.available = true;
            version
        })
        .collect::<Vec<_>>();
    select_best_version(&local_versions, Some(constraint)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn conflicting_selector_values_are_structurally_incompatible() {
        let placements = vec![
            WorkerPlacement {
                selector: BTreeMap::from([("zone".to_string(), "edge".to_string())]),
                ..Default::default()
            },
            WorkerPlacement {
                selector: BTreeMap::from([("zone".to_string(), "internal".to_string())]),
                ..Default::default()
            },
        ];
        assert_eq!(
            structural_placement_compatibility(&placements),
            StructuralPlacementCompatibility::Incompatible
        );
    }
}
