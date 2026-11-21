use crate::models::{
    ActionExecutableSnapshot, Id, Pack, PackRelease, Sensor, Worker, WorkerStatus,
};
use crate::runtime_lifecycle::{
    InstalledPreparation, PreparationScope, PreparationTarget, PreparationWorkload, RuntimeRecipe,
    MAX_TARGETS, PREPARATION_TRIGGER,
};
use crate::scheduling::{
    worker_labels_from_capabilities, worker_matches_all_placements,
    worker_taints_from_capabilities, WorkerPlacement,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, PgConnection, PgPool, Postgres};
use std::collections::HashMap;

use super::{PackInstallRepository, SensorAdmissionRepository, WorkerRepository};

pub const ATTEMPT_SECONDS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparationRequest {
    pub target: PreparationTarget,
    pub recipe: RuntimeRecipe,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PreparationAttempt {
    pub release_digest: String,
    pub requests: sqlx::types::Json<Vec<PreparationRequest>>,
    pub scopes: sqlx::types::Json<Vec<PreparationScope>>,
}

pub struct PreparationPlan {
    pub requests: Vec<PreparationRequest>,
    pub scopes: Vec<PreparationScope>,
}

#[derive(sqlx::FromRow)]
struct SensorPreparation {
    id: Id,
    digest: String,
    snapshot: serde_json::Value,
    prepared: Option<sqlx::types::Json<InstalledPreparation>>,
}

pub struct RuntimePreparationRepository;

impl RuntimePreparationRepository {
    /// Read the same immutable executable snapshots consumed by dispatch, after
    /// they have been staged in the activation transaction.
    pub async fn plan(
        connection: &mut PgConnection,
        pack: &Pack,
        release: &PackRelease,
    ) -> Result<PreparationPlan> {
        let snapshots: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT component_kind, snapshot FROM pack_release_executable WHERE release = $1 \
             AND snapshot->'runtime'->'execution_config'->'lifecycle'->'prepare_at_install' = 'true'::jsonb \
             ORDER BY component_kind, component_ref",
        )
        .bind(release.id)
        .fetch_all(&mut *connection)
        .await?;
        let mut requests = Vec::new();
        if snapshots.is_empty() {
            return Ok(PreparationPlan {
                requests,
                scopes: Vec::new(),
            });
        }
        if snapshots.len() > MAX_TARGETS {
            return Err(Error::validation(
                "The release exceeds the install-time preparation target limit",
            ));
        }
        let builders = WorkerRepository::find_action_workers(&mut *connection).await?;
        let mut scopes = builders
            .iter()
            .filter(|worker| live(worker))
            .filter_map(PreparationScope::for_worker)
            .collect::<Vec<_>>();
        scopes.sort_unstable();
        scopes.dedup();
        let pack_placement = WorkerPlacement {
            selector: pack.worker_selector_labels(),
            tolerations: pack.worker_toleration_specs(),
            affinity: pack.worker_affinity_spec(),
        };
        for (kind, snapshot) in snapshots {
            let (target, recipe, reachable) = if kind == "action" {
                let snapshot: ActionExecutableSnapshot = serde_json::from_value(snapshot)?;
                let runtime = snapshot
                    .runtime
                    .ok_or_else(|| Error::validation("Action runtime is unavailable"))?;
                let recipe = runtime
                    .parsed_execution_config()
                    .lifecycle
                    .ok_or_else(|| Error::validation("Invalid runtime preparation recipe"))?;
                let action = snapshot.action;
                let Some(target) = PreparationTarget::for_action(&action, &runtime, &recipe)
                    .map_err(recipe_error)?
                else {
                    continue;
                };
                if !snapshot.runtime_versions.is_empty()
                    || !action.required_worker_runtime_constraints().is_empty()
                {
                    return Err(Error::validation("Install-prepared actions currently require a default recipe without registered version variants or additional worker runtime constraints"));
                }
                let placement = [
                    pack_placement.clone(),
                    WorkerPlacement {
                        selector: action.worker_selector_labels(),
                        tolerations: action.worker_toleration_specs(),
                        affinity: action.worker_affinity_spec(),
                    },
                ];
                let reachable: Vec<_> = builders
                    .iter()
                    .filter(|worker| live(worker) && worker_matches_target(worker, &target))
                    .filter(|worker| {
                        worker_matches_all_placements(
                            &worker_labels_from_capabilities(worker.capabilities.as_ref()),
                            &worker_taints_from_capabilities(worker.capabilities.as_ref()),
                            &placement,
                        )
                    })
                    .filter_map(PreparationScope::for_worker)
                    .collect();
                (target, recipe, reachable)
            } else {
                let sensor: Sensor = serde_json::from_value(snapshot["sensor"].clone())?;
                let runtime = serde_json::from_value(snapshot["runtime"].clone())?;
                let recipe = serde_json::from_value(
                    snapshot["runtime"]["execution_config"]["lifecycle"].clone(),
                )?;
                let Some(target) = PreparationTarget::for_sensor(&sensor, &runtime, &recipe)
                    .map_err(recipe_error)?
                else {
                    continue;
                };
                if snapshot["runtime_versions"]
                    .as_array()
                    .is_some_and(|versions| !versions.is_empty())
                {
                    return Err(Error::validation("Install-prepared sensors currently require a default recipe without registered version variants"));
                }
                let reachable =
                    SensorAdmissionRepository::preparation_scopes(connection, sensor.id, &target)
                        .await?;
                (target, recipe, reachable)
            };
            scopes.retain(|scope| reachable.contains(scope));
            requests.push(PreparationRequest { target, recipe });
        }
        if !requests.is_empty() {
            scopes.retain(|scope| {
                builders.iter().any(|worker| {
                    live(worker)
                        && PreparationScope::for_worker(worker).as_ref() == Some(scope)
                        && requests
                            .iter()
                            .all(|request| worker_admits(worker, &request.target.recipe_digest))
                })
            });
            if scopes.is_empty() {
                return Err(Error::validation("No live runtime preparation storage scope can build and run every required action and sensor in this release"));
            }
        }
        Ok(PreparationPlan { requests, scopes })
    }

    pub async fn create_attempt(
        connection: &mut PgConnection,
        install: Id,
        activation: Id,
        release_digest: &str,
        plan: &PreparationPlan,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO pack_runtime_preparation_attempt (install, activation, release_digest, requests, scopes, expires_at) \
             VALUES ($1, $2, $3, $4, $5, clock_timestamp() + $6 * interval '1 second')",
        )
        .bind(install)
        .bind(activation)
        .bind(release_digest)
        .bind(sqlx::types::Json(&plan.requests))
        .bind(sqlx::types::Json(&plan.scopes))
        .bind(ATTEMPT_SECONDS)
        .execute(connection)
        .await?;
        Ok(())
    }

    pub async fn dispatch_scopes(pool: &PgPool, install: Id) -> Result<Vec<PreparationScope>> {
        let scopes: Option<sqlx::types::Json<Vec<PreparationScope>>> = sqlx::query_scalar(
            "SELECT a.scopes FROM pack_runtime_preparation_attempt a JOIN pack_install parent ON a.activation = parent.id \
             WHERE a.install = $1 AND a.expires_at > clock_timestamp() AND parent.status = 'activating'",
        ).bind(install).fetch_optional(pool).await?;
        Ok(scopes.map(|scopes| scopes.0).unwrap_or_default())
    }

    pub async fn begin(
        pool: &PgPool,
        install: Id,
        worker: Id,
        incarnation: uuid::Uuid,
        scope: &PreparationScope,
    ) -> Result<Option<PreparationAttempt>> {
        Ok(sqlx::query_as(
            "UPDATE pack_runtime_preparation_attempt a SET builder_incarnation = $3 \
             FROM pack_install i, pack_install parent, worker w \
             WHERE a.install = $1 AND i.id = a.install AND i.trigger_reason = $5 \
             AND i.status = 'running' AND i.assigned_worker_id = $2 AND i.candidate_digest = a.release_digest \
             AND parent.id = a.activation AND parent.status = 'activating' \
             AND w.id = $2 AND w.meta->>'registration_incarnation' = $3::uuid::text \
             AND a.builder_incarnation IS NULL AND a.expires_at > clock_timestamp() \
             AND a.scopes @> $4 RETURNING a.release_digest, a.requests, a.scopes",
        )
        .bind(install)
        .bind(worker)
        .bind(incarnation)
        .bind(serde_json::json!([scope]))
        .bind(PREPARATION_TRIGGER)
        .fetch_optional(pool)
        .await?)
    }

    pub async fn is_running(
        pool: &PgPool,
        install: Id,
        worker: Id,
        incarnation: uuid::Uuid,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pack_runtime_preparation_attempt a \
             JOIN pack_install i ON i.id = a.install JOIN pack_install parent ON parent.id = a.activation \
             JOIN worker w ON w.id = i.assigned_worker_id \
             WHERE i.id = $1 AND w.id = $2 AND a.builder_incarnation = $3 \
             AND w.meta->>'registration_incarnation' = $3::uuid::text \
             AND i.status = 'running' AND parent.status = 'activating' \
             AND a.expires_at > clock_timestamp())",
        ).bind(install).bind(worker).bind(incarnation).fetch_one(pool).await?)
    }

    /// The ready evidence and candidate completion have one transaction. A
    /// stale delivery, cancellation, or replacement worker cannot publish.
    pub async fn finish(
        pool: &PgPool,
        install: Id,
        worker: Id,
        incarnation: uuid::Uuid,
        prepared: &[InstalledPreparation],
    ) -> Result<bool> {
        let mut tx = pool.begin().await?;
        let owner: Option<Id> = sqlx::query_scalar(
            "SELECT w.id FROM pack_runtime_preparation_attempt a
             JOIN pack_install parent ON parent.id = a.activation
             JOIN worker w ON w.id = $2
             WHERE a.install = $1 AND parent.status = 'activating'
             AND w.meta->>'registration_incarnation' = $3::uuid::text
             FOR SHARE OF parent, w",
        )
        .bind(install)
        .bind(worker)
        .bind(incarnation)
        .fetch_optional(&mut *tx)
        .await?;
        if owner.is_none() {
            return Ok(false);
        }
        let Some(attempt) = PackInstallRepository::find_by_id_for_update(&mut tx, install).await?
        else {
            return Ok(false);
        };
        if attempt.status != "running" || attempt.assigned_worker_id != Some(worker) {
            return Ok(false);
        }
        let claim: Option<PreparationAttempt> = sqlx::query_as(
            "SELECT release_digest, requests, scopes FROM pack_runtime_preparation_attempt
             WHERE install = $1 AND builder_incarnation = $2",
        )
        .bind(install)
        .bind(incarnation)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(claim) = claim else {
            return Ok(false);
        };
        if !matches_plan(
            prepared,
            &claim.release_digest,
            &claim.requests,
            &claim.scopes,
        ) {
            return Err(Error::validation(
                "Runtime preparation evidence does not match its worker claim",
            ));
        }
        let affected = sqlx::query(
            "UPDATE pack_runtime_preparation_attempt a SET prepared = $4 \
             FROM pack_install parent, worker w \
             WHERE a.install = $1 AND w.id = $2 AND a.builder_incarnation = $3 \
             AND w.meta->>'registration_incarnation' = $3::uuid::text \
             AND parent.id = a.activation AND parent.status = 'activating' \
             AND a.prepared IS NULL AND a.expires_at > clock_timestamp()",
        )
        .bind(install)
        .bind(worker)
        .bind(incarnation)
        .bind(sqlx::types::Json(prepared))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected != 1 {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE pack_install SET status = 'activating', candidate_access_token_hash = NULL, \
             updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(install)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn bind_release(
        connection: &mut PgConnection,
        install: Id,
        activation: Id,
        release: &PackRelease,
        plan: &PreparationPlan,
    ) -> Result<()> {
        let result: Option<sqlx::types::Json<Vec<InstalledPreparation>>> = sqlx::query_scalar(
            "SELECT a.prepared FROM pack_runtime_preparation_attempt a \
             JOIN pack_install i ON i.id = a.install JOIN pack_install parent ON parent.id = a.activation \
             WHERE a.install = $1 AND a.activation = $2 AND a.release_digest = $3 \
             AND i.candidate_digest = a.release_digest AND i.status = 'activating' \
             AND parent.status = 'activating' AND a.prepared IS NOT NULL
             AND EXISTS (SELECT 1 FROM pack WHERE id = $4 AND active_release = $5)
             FOR UPDATE OF i",
        ).bind(install).bind(activation).bind(&release.digest).bind(release.pack).bind(release.id)
        .fetch_optional(&mut *connection).await?;
        let prepared = result.ok_or_else(|| {
            Error::validation("Release preparation did not publish ready evidence")
        })?;
        if !matches_plan(&prepared, &release.digest, &plan.requests, &plan.scopes) {
            return Err(Error::validation(
                "Release preparation targets or storage eligibility changed during installation",
            ));
        }
        for ready in &prepared.0 {
            sqlx::query(
                "INSERT INTO pack_release_runtime_preparation (release, component_kind, component_ref, prepared) VALUES ($1, $2, $3, $4)",
            ).bind(release.id).bind(kind_name(ready.target.kind)).bind(&ready.target.reference).bind(sqlx::types::Json(ready))
            .execute(&mut *connection).await?;
        }
        PackInstallRepository::finish_activation_in_transaction(connection, install, release.pack)
            .await?
            .ok_or_else(|| Error::validation("Runtime preparation activation was cancelled"))?;
        Ok(())
    }

    pub async fn ready_for_sensors(
        connection: &mut PgConnection,
        sensors: &[Id],
    ) -> Result<HashMap<Id, Vec<InstalledPreparation>>> {
        let rows: Vec<SensorPreparation> =
            sqlx::query_as(
                "SELECT s.id, r.digest, e.snapshot, p.prepared FROM sensor s
                 JOIN pack_release r ON r.id = s.managed_release
                 JOIN pack_release_executable e ON e.release = r.id
                    AND e.component_kind = 'sensor' AND e.component_id = s.id
                 LEFT JOIN pack_release_runtime_preparation p ON p.release = r.id
                    AND p.component_kind = 'sensor' AND p.component_ref = s.ref
                 WHERE s.id = ANY($1)
                 AND e.snapshot->'runtime'->'execution_config'->'lifecycle'->'prepare_at_install' = 'true'::jsonb",
            )
            .bind(sensors)
            .fetch_all(connection)
            .await?;
        let mut ready = HashMap::<Id, Vec<InstalledPreparation>>::new();
        for SensorPreparation {
            id,
            digest,
            snapshot,
            prepared,
        } in rows
        {
            let records = ready.entry(id).or_default();
            if let Some(candidate) = prepared {
                let sensor = serde_json::from_value(snapshot["sensor"].clone())?;
                let runtime = serde_json::from_value(snapshot["runtime"].clone())?;
                let recipe = serde_json::from_value(
                    snapshot["runtime"]["execution_config"]["lifecycle"].clone(),
                )?;
                let target = PreparationTarget::for_sensor(&sensor, &runtime, &recipe)
                    .map_err(recipe_error)?;
                if target.as_ref() == Some(&candidate.target) && candidate.release_digest == digest
                {
                    records.push(candidate.0);
                }
            }
        }
        Ok(ready)
    }

    pub async fn ready_for<'a>(
        executor: impl Executor<'a, Database = Postgres>,
        release: Id,
        digest: &str,
        target: &PreparationTarget,
    ) -> Result<Vec<InstalledPreparation>> {
        let records: Vec<sqlx::types::Json<InstalledPreparation>> = sqlx::query_scalar(
            "SELECT prepared FROM pack_release_runtime_preparation WHERE release = $1 AND component_kind = $2 \
             AND component_ref = $3 ORDER BY id DESC",
        ).bind(release).bind(kind_name(target.kind)).bind(&target.reference).fetch_all(executor).await?;
        Ok(records
            .into_iter()
            .map(|record| record.0)
            .filter(|record| record.release_digest == digest && record.target == *target)
            .collect())
    }
}

fn matches_plan(
    prepared: &[InstalledPreparation],
    release_digest: &str,
    requests: &[PreparationRequest],
    scopes: &[PreparationScope],
) -> bool {
    let Some(first) = prepared.first() else {
        return false;
    };
    scopes.contains(&first.scope)
        && prepared.len() == requests.len()
        && prepared.iter().zip(requests).all(|(ready, request)| {
            ready.scope == first.scope
                && ready.target == request.target
                && ready.release_digest == release_digest
        })
}

pub fn worker_admits(worker: &Worker, digest: &str) -> bool {
    worker
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.get("runtime_recipe_digests"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|digests| digests.iter().any(|value| value.as_str() == Some(digest)))
}

pub fn worker_matches_target(worker: &Worker, target: &PreparationTarget) -> bool {
    worker_admits(worker, &target.recipe_digest)
        && worker
            .capabilities
            .as_ref()
            .and_then(|caps| caps.get("runtime_preparation_filter"))
            .is_none_or(|filter| {
                serde_json::from_value::<Vec<String>>(filter.clone()).is_ok_and(|filter| {
                    crate::runtime_detection::runtime_aliases_match_filter(
                        &target.runtime_names,
                        &filter,
                    )
                })
            })
}

pub fn can_consume(worker: &Worker, prepared: &[InstalledPreparation]) -> bool {
    PreparationScope::for_worker(worker).is_some_and(|scope| {
        prepared
            .iter()
            .any(|ready| ready.scope == scope && worker_matches_target(worker, &ready.target))
    })
}

fn live(worker: &Worker) -> bool {
    !worker.cordoned
        && worker.status == Some(WorkerStatus::Active)
        && worker.last_heartbeat.is_some_and(|heartbeat| {
            let age = chrono::Utc::now()
                .signed_duration_since(heartbeat)
                .num_seconds();
            (0..90).contains(&age)
        })
}

fn kind_name(kind: PreparationWorkload) -> &'static str {
    match kind {
        PreparationWorkload::Action => "action",
        PreparationWorkload::Sensor => "sensor",
    }
}

fn recipe_error(error: crate::runtime_lifecycle::Error) -> Error {
    Error::validation(error.to_string())
}
