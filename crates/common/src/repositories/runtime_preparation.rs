use crate::models::{
    ActionExecutableSnapshot, Id, Pack, PackRelease, Sensor, Worker, WorkerRole, WorkerStatus,
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

use super::{FindById, PackInstallRepository, SensorAdmissionRepository, WorkerRepository};

pub const ATTEMPT_SECONDS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparationJob {
    pub target: PreparationTarget,
    pub recipe: RuntimeRecipe,
    pub scopes: Vec<PreparationScope>,
    pub placements: Vec<WorkerPlacement>,
}

impl PreparationJob {
    fn builder_scope(&self, worker: &Worker) -> Option<PreparationScope> {
        (worker.worker_role == WorkerRole::Action
            && live(worker)
            && worker_matches_target(worker, &self.target)
            && worker_matches_all_placements(
                &worker_labels_from_capabilities(worker.capabilities.as_ref()),
                &worker_taints_from_capabilities(worker.capabilities.as_ref()),
                &self.placements,
            ))
        .then(|| PreparationScope::for_worker(worker))
        .flatten()
    }

    pub fn can_build(&self, worker: &Worker) -> bool {
        self.builder_scope(worker)
            .is_some_and(|scope| self.scopes.contains(&scope))
    }

    fn matches_ready(&self, release_digest: &str, ready: &InstalledPreparation) -> bool {
        ready.target == self.target
            && ready.release_digest == release_digest
            && self.scopes.contains(&ready.scope)
            && ready.scope.is_object_backed() == ready.archive().is_some()
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct PreparationAttempt {
    pub release_digest: String,
    pub job: sqlx::types::Json<PreparationJob>,
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
    ) -> Result<Vec<PreparationJob>> {
        let snapshots: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT component_kind, snapshot FROM pack_release_executable WHERE release = $1 \
             AND snapshot->'runtime'->'execution_config'->'lifecycle'->'prepare_at_install' = 'true'::jsonb \
             ORDER BY component_kind, component_ref",
        )
        .bind(release.id)
        .fetch_all(&mut *connection)
        .await?;
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }
        if snapshots.len() > MAX_TARGETS {
            return Err(Error::validation(
                "The release exceeds the install-time preparation target limit",
            ));
        }
        let builders = WorkerRepository::find_pack_workers(&mut *connection, false).await?;
        let mut jobs = Vec::with_capacity(snapshots.len());
        let pack_placement = WorkerPlacement {
            selector: pack.worker_selector_labels(),
            tolerations: pack.worker_toleration_specs(),
            affinity: pack.worker_affinity_spec(),
        };
        for (kind, snapshot) in snapshots {
            let (target, recipe, placements, sensor_scopes) = if kind == "action" {
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
                let placements = vec![
                    pack_placement.clone(),
                    WorkerPlacement {
                        selector: action.worker_selector_labels(),
                        tolerations: action.worker_toleration_specs(),
                        affinity: action.worker_affinity_spec(),
                    },
                ];
                (target, recipe, placements, None)
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
                // Sensor and rule placement govern sensor consumers. The builder
                // is an action worker and must satisfy the pack placement.
                (
                    target,
                    recipe,
                    vec![pack_placement.clone()],
                    Some(reachable),
                )
            };
            let mut job = PreparationJob {
                target,
                recipe,
                scopes: Vec::new(),
                placements,
            };
            job.scopes = builders
                .iter()
                .filter_map(|worker| job.builder_scope(worker))
                .filter(|scope| {
                    sensor_scopes
                        .as_ref()
                        .is_none_or(|scopes| scopes.contains(scope))
                })
                .collect();
            job.scopes.sort_unstable();
            job.scopes.dedup();
            if job.scopes.is_empty() {
                return Err(Error::validation(format!(
                    "No live runtime preparation storage scope can build and run '{}'",
                    job.target.reference,
                )));
            }
            jobs.push(job);
        }
        Ok(jobs)
    }

    pub async fn create_attempt(
        connection: &mut PgConnection,
        install: Id,
        activation: Id,
        release_digest: &str,
        job: &PreparationJob,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO pack_runtime_preparation_attempt (install, activation, release_digest, job, expires_at) \
             VALUES ($1, $2, $3, $4, clock_timestamp() + $5 * interval '1 second')",
        )
        .bind(install)
        .bind(activation)
        .bind(release_digest)
        .bind(sqlx::types::Json(job))
        .bind(ATTEMPT_SECONDS)
        .execute(connection)
        .await?;
        Ok(())
    }

    pub async fn dispatch_job<'e>(
        connection: impl Executor<'e, Database = Postgres>,
        install: Id,
    ) -> Result<Option<PreparationJob>> {
        let job: Option<sqlx::types::Json<PreparationJob>> = sqlx::query_scalar(
            "SELECT a.job FROM pack_runtime_preparation_attempt a JOIN pack_install parent ON a.activation = parent.id
             WHERE a.install = $1 AND a.expires_at > clock_timestamp() AND parent.status = 'activating'",
        ).bind(install).fetch_optional(connection).await?;
        Ok(job.map(|job| job.0))
    }

    pub async fn begin(
        pool: &PgPool,
        install: Id,
        worker: Id,
        incarnation: uuid::Uuid,
        scope: &PreparationScope,
    ) -> Result<Option<PreparationAttempt>> {
        let mut tx = pool.begin().await?;
        let attempt = Self::begin_with(&mut tx, install, worker, incarnation, scope).await?;
        tx.commit().await?;
        Ok(attempt)
    }

    pub async fn begin_with(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        install: Id,
        worker: Id,
        incarnation: uuid::Uuid,
        scope: &PreparationScope,
    ) -> Result<Option<PreparationAttempt>> {
        let Some(job) = Self::dispatch_job(&mut **tx, install).await? else {
            return Ok(None);
        };
        let Some(builder) = WorkerRepository::find_registration_for_share(tx, worker).await? else {
            return Ok(None);
        };
        if !job.can_build(&builder)
            || PreparationScope::for_worker(&builder).as_ref() != Some(scope)
        {
            return Ok(None);
        }
        let attempt = sqlx::query_as(
            "UPDATE pack_runtime_preparation_attempt a SET builder_incarnation = $3, builder_scope = $5
             FROM pack_install i, pack_install parent, worker w
             WHERE a.install = $1 AND i.id = a.install AND i.trigger_reason = $4
             AND i.status = 'running' AND i.assigned_worker_id = $2 AND i.candidate_digest = a.release_digest
             AND parent.id = a.activation AND parent.status = 'activating'
             AND w.id = $2 AND w.meta->>'registration_incarnation' = $3::uuid::text
             AND a.builder_incarnation IS NULL AND a.expires_at > clock_timestamp()
             RETURNING a.release_digest, a.job",
        )
        .bind(install)
        .bind(worker)
        .bind(incarnation)
        .bind(PREPARATION_TRIGGER)
        .bind(sqlx::types::Json(scope))
        .fetch_optional(&mut **tx)
        .await?;
        Ok(attempt)
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
        prepared: &InstalledPreparation,
    ) -> Result<bool> {
        let mut tx = pool.begin().await?;
        let finished = Self::finish_with(&mut tx, install, worker, incarnation, prepared).await?;
        if finished {
            tx.commit().await?;
        }
        Ok(finished)
    }

    pub async fn finish_with(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        install: Id,
        worker: Id,
        incarnation: uuid::Uuid,
        prepared: &InstalledPreparation,
    ) -> Result<bool> {
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
        .fetch_optional(&mut **tx)
        .await?;
        if owner.is_none() {
            return Ok(false);
        }
        let Some(attempt) = PackInstallRepository::find_by_id_for_update(tx, install).await? else {
            return Ok(false);
        };
        if attempt.status != "running" || attempt.assigned_worker_id != Some(worker) {
            return Ok(false);
        }
        let claim: Option<PreparationAttempt> = sqlx::query_as(
            "SELECT release_digest, job FROM pack_runtime_preparation_attempt
             WHERE install = $1 AND builder_incarnation = $2",
        )
        .bind(install)
        .bind(incarnation)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(claim) = claim else {
            return Ok(false);
        };
        if !claim.job.matches_ready(&claim.release_digest, prepared) {
            return Err(Error::validation(
                "Runtime preparation evidence does not match its worker claim",
            ));
        }
        let affected = sqlx::query(
            "UPDATE pack_runtime_preparation_attempt a SET prepared = $4 \
             FROM pack_install parent, worker w \
             WHERE a.install = $1 AND w.id = $2 AND a.builder_incarnation = $3 \
             AND a.builder_scope = $5 AND w.meta->>'registration_incarnation' = $3::uuid::text \
             AND parent.id = a.activation AND parent.status = 'activating' \
             AND a.prepared IS NULL AND a.expires_at > clock_timestamp()",
        )
        .bind(install)
        .bind(worker)
        .bind(incarnation)
        .bind(sqlx::types::Json(prepared))
        .bind(sqlx::types::Json(&prepared.scope))
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if affected != 1 {
            return Ok(false);
        }
        if let Some(archive) = prepared.archive() {
            super::ObjectMaintenanceRepository::record_uploaded(
                &mut **tx,
                &archive.key,
                &archive.version,
                i64::try_from(archive.size)
                    .map_err(|_| Error::validation("Invalid archive size"))?,
            )
            .await?;
        }
        sqlx::query(
            "UPDATE pack_install SET status = 'activating', candidate_access_token_hash = NULL, \
             updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(install)
        .execute(&mut **tx)
        .await?;
        Ok(true)
    }

    /// The HTTP worker token identifies a delivery instance. Translate that
    /// registered instance to the separate preparation claim incarnation.
    pub async fn builder_for_upload(
        pool: &PgPool,
        install: Id,
        worker: Id,
        instance: uuid::Uuid,
    ) -> Result<Option<uuid::Uuid>> {
        Ok(sqlx::query_scalar(
            "SELECT a.builder_incarnation
             FROM pack_runtime_preparation_attempt a
             JOIN pack_install i ON i.id = a.install
             JOIN pack_install parent ON parent.id = a.activation
             JOIN worker w ON w.id = i.assigned_worker_id
             WHERE a.install = $1 AND w.id = $2
                AND w.capabilities->>'log_delivery_instance' = $3::uuid::text
                AND w.meta->>'registration_incarnation' = a.builder_incarnation::text
                AND i.status = 'running' AND parent.status = 'activating'
                AND a.expires_at > clock_timestamp() AND a.prepared IS NULL",
        )
        .bind(install)
        .bind(worker)
        .bind(instance)
        .fetch_optional(pool)
        .await?)
    }

    pub async fn bind_release(
        connection: &mut PgConnection,
        installs: &[Id],
        activation: Id,
        release: &PackRelease,
        jobs: &[PreparationJob],
    ) -> Result<()> {
        let prepared: Vec<(Id, sqlx::types::Json<InstalledPreparation>)> = sqlx::query_as(
            "SELECT i.id, a.prepared FROM pack_runtime_preparation_attempt a
             JOIN pack_install i ON i.id = a.install JOIN pack_install parent ON parent.id = a.activation
             WHERE a.install = ANY($1) AND a.activation = $2 AND a.release_digest = $3
             AND i.candidate_digest = a.release_digest AND i.status = 'activating'
             AND parent.status = 'activating' AND a.prepared IS NOT NULL
             AND EXISTS (SELECT 1 FROM pack WHERE id = $4 AND active_release = $5)
             ORDER BY i.id FOR UPDATE OF i",
        ).bind(installs).bind(activation).bind(&release.digest).bind(release.pack).bind(release.id)
        .fetch_all(&mut *connection).await?;

        // Verify the entire set against the freshly projected release before
        // writing any readiness rows. Each target can use its own scope.
        if installs.len() != jobs.len()
            || prepared.len() != jobs.len()
            || !jobs.iter().all(|job| {
                prepared
                    .iter()
                    .any(|(_, ready)| job.matches_ready(&release.digest, ready))
            })
        {
            return Err(Error::validation(
                "Release preparation evidence is incomplete or storage eligibility changed during installation",
            ));
        }
        for (install, ready) in prepared {
            sqlx::query(
                "INSERT INTO pack_release_runtime_preparation (release, component_kind, component_ref, prepared) VALUES ($1, $2, $3, $4)",
            ).bind(release.id).bind(kind_name(ready.target.kind)).bind(&ready.target.reference).bind(&ready)
            .execute(&mut *connection).await?;
            PackInstallRepository::finish_activation_in_transaction(
                connection,
                install,
                release.pack,
            )
            .await?
            .ok_or_else(|| Error::validation("Runtime preparation activation was cancelled"))?;
        }
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

    /// Only a registered, compatible manager can download release-bound output.
    pub async fn find_download(
        pool: &PgPool,
        worker: Id,
        release: Id,
        directory: uuid::Uuid,
    ) -> Result<Option<InstalledPreparation>> {
        let Some(worker) = WorkerRepository::find_by_id(pool, worker).await? else {
            return Ok(None);
        };
        Self::find_download_with(pool, &worker, release, directory).await
    }

    pub async fn find_download_with<'e>(
        connection: impl Executor<'e, Database = Postgres>,
        worker: &Worker,
        release: Id,
        directory: uuid::Uuid,
    ) -> Result<Option<InstalledPreparation>> {
        let ready: Option<sqlx::types::Json<InstalledPreparation>> = sqlx::query_scalar(
            "SELECT p.prepared FROM pack_release_runtime_preparation p
             JOIN pack_release r ON r.id = p.release
             WHERE p.release = $1 AND p.prepared->>'directory' = $2
                 AND p.prepared->>'release_digest' = r.digest
             ORDER BY p.id DESC LIMIT 1",
        )
        .bind(release)
        .bind(directory.to_string())
        .fetch_optional(connection)
        .await?;
        Ok(ready.map(|ready| ready.0).filter(|ready| {
            ready.archive().is_some() && can_consume(worker, std::slice::from_ref(ready))
        }))
    }
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
