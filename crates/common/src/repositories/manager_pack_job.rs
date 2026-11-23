use chrono::{DateTime, Utc};
use sqlx::{types::Json, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    auth::hash_integration_token,
    manager_control::{
        ClaimPackJob, ClaimedPackJob, FinishPackJob, PackJobEffects, PackJobOffer, PackJobSource,
    },
    models::{Id, PackInstall, PackInstallStatus, Runtime, Worker, WorkerRole},
    mq::PackTestRequestedPayload,
    runtime_detection::normalize_runtime_name,
    runtime_lifecycle::{InstalledPreparation, PreparationScope, PREPARATION_TRIGGER},
    test_executor::TestConfig,
    Error, Result,
};

use super::{
    candidate_effect::{CandidateArchiveBinding, CandidateEffectRepository},
    manager_assignment::MANAGER_WORK_WAKEUP,
    manager_runtime::ManagerRuntimeRepository,
    manager_session::ManagerSessionRecord,
    pack_install::PACK_INSTALL_ACTIVE_TTL_SECS,
    runtime_preparation::RuntimePreparationRepository,
    FindByRef, PackInstallRepository, PackReleaseRepository, PackRepository, PackTestRepository,
    RuntimeRepository, WorkerRepository,
};

const LEASE_SECONDS: i64 = 40;
const MAX_ATTEMPTS: i32 = 3;
const JOB_COLUMNS: &str = "install, payload, worker, manager_session, instance, attempt, \
    release, release_digest, deadline, claim, finish_digest, python";

#[derive(sqlx::FromRow)]
pub struct ManagerPackJob {
    pub install: Id,
    payload: Json<PackTestRequestedPayload>,
    worker: Option<Id>,
    manager_session: Option<Id>,
    instance: Option<Uuid>,
    attempt: i32,
    release: Option<Id>,
    release_digest: Option<String>,
    deadline: DateTime<Utc>,
    claim: Option<Uuid>,
    finish_digest: Option<String>,
    python: Option<Json<Runtime>>,
}

pub enum PackDispatch {
    Legacy,
    Offered,
    AlreadyDispatched,
    PlacementChanged,
}

pub struct ManagerPackJobRepository;

fn denied() -> Error {
    Error::PermissionDenied("Pack job is stale or outside this manager's scope".into())
}

impl ManagerPackJobRepository {
    async fn hard_deadline(
        conn: &mut PgConnection,
        install: &PackInstall,
    ) -> Result<DateTime<Utc>> {
        // Preparation has its own earlier deadline. LEAST ignores the absent
        // preparation row for ordinary tests.
        Ok(sqlx::query_scalar(
            "SELECT LEAST($2::timestamptz, (SELECT expires_at FROM pack_runtime_preparation_attempt WHERE install=$1))",
        )
        .bind(install.id)
        .bind(install.started_at + chrono::Duration::seconds(PACK_INSTALL_ACTIVE_TTL_SECS))
        .fetch_one(conn).await?)
    }

    /// Called by the executor after ordinary placement. The install and HTTP
    /// offer commit together; a manager-bound worker never gets a broker copy.
    pub async fn offer(
        pool: &PgPool,
        selected: &Worker,
        payload: &PackTestRequestedPayload,
    ) -> Result<PackDispatch> {
        let mut tx = pool.begin().await?;
        let binding: Option<(Id, Uuid)> = sqlx::query_as(
            "SELECT s.id, s.instance FROM manager_action_worker m JOIN manager_session s ON s.id=m.manager_session WHERE m.worker=$1",
        ).bind(selected.id).fetch_optional(&mut *tx).await?;
        let Some((manager, instance)) = binding else {
            return Ok(PackDispatch::Legacy);
        };
        // Use the release retention guard before the catalog and worker locks.
        // Activation uses the same order before it publishes changed runtimes.
        if payload.candidate_path.is_none() {
            super::schema_type::SchemaTypeRepository::lock_selection(&mut tx).await?;
        }
        ManagerRuntimeRepository::lock_revision(&mut tx).await?;
        let active: bool = sqlx::query_scalar(
            "SELECT closed_at IS NULL AND expires_at > clock_timestamp() FROM manager_session
             WHERE id=$1 AND instance=$2 AND role='action' FOR SHARE",
        )
        .bind(manager)
        .bind(instance)
        .fetch_one(&mut *tx)
        .await?;
        if !active {
            return Ok(PackDispatch::PlacementChanged);
        }
        if !ManagerRuntimeRepository::retain_placement(&mut tx, selected).await? {
            return Ok(PackDispatch::PlacementChanged);
        }
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM manager_action_worker m JOIN manager_session s ON s.id=m.manager_session
              WHERE m.worker=$1 AND m.manager_session=$2 AND s.instance=$3 AND NOT m.draining
                AND s.closed_at IS NULL AND s.expires_at > clock_timestamp())",
        ).bind(selected.id).bind(manager).bind(instance).fetch_one(&mut *tx).await?;
        if !eligible {
            return Ok(PackDispatch::PlacementChanged);
        }
        let Some(install) = PackInstallRepository::claim_worker_with(
            &mut *tx,
            payload.pack_install_id,
            selected.id,
            None,
        )
        .await?
        else {
            return Ok(PackDispatch::AlreadyDispatched);
        };
        let candidate = payload.candidate_path.is_some();
        if candidate != install.candidate_digest.is_some()
            || payload.candidate_api_effects && !candidate
        {
            return Err(Error::validation("Pack job archive binding is incomplete"));
        }
        let previous: Option<(Option<Id>, Option<String>)> =
            sqlx::query_as("SELECT release, release_digest FROM manager_pack_job WHERE install=$1")
                .bind(install.id)
                .fetch_optional(&mut *tx)
                .await?;
        let (release, digest) = if let Some((Some(release), Some(digest))) = previous {
            (Some(release), Some(digest))
        } else if candidate {
            (None, None)
        } else {
            let release =
                PackReleaseRepository::find_active_by_pack_ref(&mut *tx, &install.pack_ref)
                    .await?
                    .filter(|release| release.version == install.pack_version)
                    .ok_or_else(|| {
                        Error::validation("Pack test release changed before dispatch")
                    })?;
            (Some(release.id), Some(release.digest))
        };
        let python = if payload
            .required_runtimes
            .iter()
            .any(|name| normalize_runtime_name(name) == "python")
        {
            RuntimeRepository::find_by_ref(&mut *tx, "core.python")
                .await?
                .map(Json)
        } else {
            None
        };
        let mut payload = payload.clone();
        payload.candidate_path = candidate.then(|| "candidate".into());
        payload.candidate_access_token = None;
        // Queued work can wait while a healthy manager runs an earlier job.
        // The short execution lease starts when the manager claims it.
        let deadline = Self::hard_deadline(&mut tx, &install).await?;
        sqlx::query(
            "INSERT INTO manager_pack_job (install, payload, worker, manager_session, instance, attempt, release, release_digest, deadline, python)
             VALUES ($1,$2,$3,$4,$5,1,$6,$7,$8,$9)
             ON CONFLICT (install) DO UPDATE SET worker=EXCLUDED.worker, manager_session=EXCLUDED.manager_session,
                 instance=EXCLUDED.instance, deadline=EXCLUDED.deadline, release=EXCLUDED.release,
                 release_digest=EXCLUDED.release_digest, python=COALESCE(manager_pack_job.python,EXCLUDED.python)
             WHERE manager_pack_job.worker IS NULL",
        ).bind(install.id).bind(Json(payload)).bind(selected.id).bind(manager).bind(instance)
            .bind(release).bind(digest).bind(deadline).bind(python).execute(&mut *tx).await?;
        sqlx::query("SELECT pg_notify($1,$2)")
            .bind(MANAGER_WORK_WAKEUP)
            .bind(manager.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(PackDispatch::Offered)
    }

    pub async fn next_offer(
        conn: &mut PgConnection,
        manager: &ManagerSessionRecord,
    ) -> Result<Option<PackJobOffer>> {
        let offer: Option<(Id, i32)> = sqlx::query_as(
            "SELECT j.install, j.attempt FROM manager_pack_job j JOIN pack_install i ON i.id=j.install
             JOIN manager_action_worker m ON m.worker=j.worker AND m.manager_session=j.manager_session
             WHERE j.manager_session=$1 AND j.instance=$2 AND j.claim IS NULL AND NOT m.draining
               AND j.deadline > clock_timestamp() AND i.status='running'
             ORDER BY j.install LIMIT 1",
        ).bind(manager.id).bind(manager.instance).fetch_optional(conn).await?;
        Ok(offer.map(|(install, attempt)| PackJobOffer { install, attempt }))
    }

    pub async fn awaiting_recovery(pool: &PgPool, install: Id) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM manager_pack_job WHERE install=$1 AND worker IS NULL)",
        )
        .bind(install)
        .fetch_one(pool)
        .await?)
    }

    pub async fn retry_placement(pool: &PgPool, payload: &PackTestRequestedPayload) -> Result<()> {
        let mut safe = payload.clone();
        safe.candidate_path = safe.candidate_path.map(|_| "candidate".into());
        safe.candidate_access_token = None;
        sqlx::query(
            "INSERT INTO manager_pack_job (install,payload,attempt,deadline)
             SELECT i.id,$2,1,clock_timestamp() FROM pack_install i WHERE i.id=$1 AND i.status='pending'
             ON CONFLICT (install) DO NOTHING",
        ).bind(payload.pack_install_id).bind(Json(safe)).execute(pool).await?;
        Ok(())
    }

    async fn lock(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        id: Id,
    ) -> Result<(ManagerPackJob, PackInstall)> {
        if manager.role != WorkerRole::Action {
            return Err(denied());
        }
        let worker = ManagerRuntimeRepository::lock_worker(tx, manager).await?;
        let install = PackInstallRepository::find_by_id_for_update(tx, id)
            .await?
            .ok_or_else(denied)?;
        let job: ManagerPackJob = sqlx::query_as(&format!(
            "SELECT {JOB_COLUMNS} FROM manager_pack_job WHERE install=$1 FOR UPDATE"
        ))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        if job.worker != Some(worker)
            || job.manager_session != Some(manager.id)
            || job.instance != Some(manager.instance)
            || install.assigned_worker_id != Some(worker)
        {
            return Err(denied());
        }
        Ok((job, install))
    }

    async fn check_running(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        expires: DateTime<Utc>,
        job: &ManagerPackJob,
        install: &PackInstall,
    ) -> Result<()> {
        if install.status != "running" {
            return Err(denied());
        }
        if job.payload.trigger_reason == PREPARATION_TRIGGER
            && RuntimePreparationRepository::dispatch_job(&mut **tx, install.id)
                .await?
                .is_none()
        {
            return Err(denied());
        }
        let hard_deadline =
            install.started_at + chrono::Duration::seconds(PACK_INSTALL_ACTIVE_TTL_SECS);
        ManagerRuntimeRepository::check_deadline(
            tx,
            manager,
            expires.min(job.deadline).min(hard_deadline),
        )
        .await
    }

    pub async fn claim(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        expires: DateTime<Utc>,
        id: Id,
        request: ClaimPackJob,
        effect_credentials: (Uuid, &str),
    ) -> Result<ClaimedPackJob> {
        let (job, install) = Self::lock(tx, manager, id).await?;
        if request.request.is_nil()
            || request.attempt != job.attempt
            || job.claim.is_some_and(|claim| claim != request.request)
        {
            return Err(denied());
        }
        Self::check_running(tx, manager, expires, &job, &install).await?;
        let worker = job.worker.ok_or_else(denied)?;
        let source = match (job.release, job.release_digest.as_ref()) {
            (Some(id), Some(digest)) => PackJobSource::Release {
                id,
                digest: digest.clone(),
            },
            _ => PackJobSource::Candidate {
                digest: install.candidate_digest.clone().ok_or_else(denied)?,
            },
        };
        let preparation = if job.payload.trigger_reason == PREPARATION_TRIGGER {
            if job.claim.is_none() {
                let registered = WorkerRepository::find_registration_for_share(tx, worker)
                    .await?
                    .ok_or_else(denied)?;
                let scope = PreparationScope::for_worker(&registered).ok_or_else(denied)?;
                Some(
                    RuntimePreparationRepository::begin_with(
                        tx,
                        id,
                        worker,
                        manager.instance,
                        &scope,
                    )
                    .await?
                    .ok_or_else(denied)?
                    .job
                    .0,
                )
            } else {
                Some(
                    RuntimePreparationRepository::dispatch_job(&mut **tx, id)
                        .await?
                        .ok_or_else(denied)?,
                )
            }
        } else {
            None
        };

        if job.claim.is_none() {
            let allowed: bool = sqlx::query_scalar(
                "SELECT NOT draining FROM manager_action_worker WHERE manager_session=$1 FOR SHARE",
            )
            .bind(manager.id)
            .fetch_optional(&mut **tx)
            .await?
            .unwrap_or(false);
            if !allowed {
                return Err(denied());
            }
            sqlx::query("UPDATE manager_pack_job SET claim=$2, deadline=LEAST(deadline,clock_timestamp()+make_interval(secs=>$3)) WHERE install=$1")
                .bind(id).bind(request.request).bind(LEASE_SECONDS as f64).execute(&mut **tx).await?;
            if matches!(source, PackJobSource::Candidate { .. }) {
                sqlx::query("UPDATE pack_install SET candidate_access_token_hash=$2 WHERE id=$1")
                    .bind(id)
                    .bind(hash_integration_token(&request.request.to_string()))
                    .execute(&mut **tx)
                    .await?;
            }
        }

        let effects = if job.payload.candidate_api_effects {
            let archive = CandidateArchiveBinding {
                key: install.candidate_object_key.as_deref().ok_or_else(denied)?,
                provider_version: install
                    .candidate_provider_version
                    .as_deref()
                    .ok_or_else(denied)?,
                digest: install.candidate_digest.as_deref().ok_or_else(denied)?,
            };
            let (lease, token) = effect_credentials;
            let id = CandidateEffectRepository::open_manager(
                tx,
                id,
                worker,
                request.request,
                lease,
                token,
                &archive,
            )
            .await?;
            Some(PackJobEffects {
                id,
                token: token.to_string(),
            })
        } else {
            None
        };
        Self::check_running(tx, manager, expires, &job, &install).await?;
        Ok(ClaimedPackJob {
            install: id,
            pack_ref: install.pack_ref,
            version: install.pack_version,
            source,
            preparation,
            python: job.python.map(|runtime| runtime.0),
            effects,
        })
    }

    pub async fn renew(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        expires: DateTime<Utc>,
        id: Id,
        claim: Uuid,
    ) -> Result<Option<DateTime<Utc>>> {
        let (job, install) = Self::scoped(tx, manager, id, claim).await?;
        if job.finish_digest.is_some() {
            return Ok(None);
        }
        Self::check_running(tx, manager, expires, &job, &install).await?;
        let hard_deadline = Self::hard_deadline(tx, &install).await?;
        let deadline = sqlx::query_scalar(
            "UPDATE manager_pack_job SET deadline=LEAST($4::timestamptz,clock_timestamp()+make_interval(secs=>$2))
             WHERE install=$1 AND LEAST(deadline,$3)>clock_timestamp() RETURNING deadline",
        )
        .bind(id)
        .bind(LEASE_SECONDS as f64)
        .bind(expires.min(manager.expires_at))
        .bind(hard_deadline)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        Ok(Some(deadline))
    }

    async fn scoped(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        id: Id,
        claim: Uuid,
    ) -> Result<(ManagerPackJob, PackInstall)> {
        let (job, install) = Self::lock(tx, manager, id).await?;
        if job.claim != Some(claim) {
            return Err(denied());
        }
        Ok((job, install))
    }

    pub async fn authorize_archive(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        expires: DateTime<Utc>,
        id: Id,
        claim: Uuid,
        release: Option<Id>,
    ) -> Result<(PackInstall, Option<String>)> {
        let (job, install) = Self::scoped(tx, manager, id, claim).await?;
        Self::check_running(tx, manager, expires, &job, &install).await?;
        if job.release != release {
            return Err(denied());
        }
        Ok((install, job.release_digest))
    }

    pub async fn prepared_upload(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        expires: DateTime<Utc>,
        id: Id,
        claim: Uuid,
        finish: Option<(&InstalledPreparation, &str)>,
    ) -> Result<Option<String>> {
        let (job, install) = Self::scoped(tx, manager, id, claim).await?;
        if job.payload.trigger_reason != PREPARATION_TRIGGER {
            return Err(denied());
        }
        ManagerRuntimeRepository::check_deadline(tx, manager, expires).await?;
        if let Some(prior) = job.finish_digest {
            if finish.is_some_and(|(_, digest)| digest != prior) || !prior.starts_with("archive:") {
                return Err(denied());
            }
            return Ok(Some(prior));
        }
        Self::check_running(tx, manager, expires, &job, &install).await?;
        if let Some((prepared, digest)) = finish {
            if !RuntimePreparationRepository::finish_with(
                tx,
                id,
                job.worker.ok_or_else(denied)?,
                manager.instance,
                prepared,
            )
            .await?
            {
                return Err(denied());
            }
            Self::receipt(tx, id, digest).await?;
            ManagerRuntimeRepository::check_deadline(tx, manager, expires.min(job.deadline))
                .await?;
        }
        Ok(None)
    }

    pub async fn finish(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        expires: DateTime<Utc>,
        id: Id,
        claim: Uuid,
        finish: FinishPackJob,
    ) -> Result<()> {
        let digest = crate::runtime_file_body::sha256(&serde_json::to_vec(&finish)?);
        let (job, install) = Self::scoped(tx, manager, id, claim).await?;
        if let Some(prior) = job.finish_digest {
            ManagerRuntimeRepository::check_deadline(tx, manager, expires).await?;
            return if prior == digest {
                Ok(())
            } else {
                Err(denied())
            };
        }
        Self::check_running(tx, manager, expires, &job, &install).await?;
        match finish {
            FinishPackJob::Tests { summary }
                if job.payload.trigger_reason != PREPARATION_TRIGGER =>
            {
                if summary.pack_ref != install.pack_ref
                    || summary.pack_version != install.pack_version
                    || !summary.pass_rate.is_finite()
                    || !(0.0..=1.0).contains(&summary.pass_rate)
                {
                    return Err(Error::validation(
                        "Pack test result does not match the claimed install",
                    ));
                }
                let accepted = TestConfig::accepts_result_with_policy(
                    &summary,
                    job.payload.test_min_pass_rate,
                    job.payload.test_allows_failure,
                );
                let mut test = None;
                if let Some(pack) = install.pack_id {
                    if PackRepository::find_by_ref(&mut **tx, &install.pack_ref)
                        .await?
                        .is_some_and(|row| row.id == pack)
                    {
                        test = Some(
                            PackTestRepository::create_with(
                                &mut **tx,
                                pack,
                                &install.pack_version,
                                &install.trigger_reason,
                                &summary,
                            )
                            .await?
                            .id,
                        );
                    }
                }
                let (status, message) = if !accepted {
                    (
                        PackInstallStatus::Failed,
                        Some("Pack tests did not meet the configured acceptance policy".into()),
                    )
                } else if install.trigger_reason == "manual" {
                    (PackInstallStatus::Succeeded, None)
                } else {
                    (PackInstallStatus::Activating, None)
                };
                PackInstallRepository::finish_running_with(
                    &mut **tx,
                    id,
                    status,
                    test,
                    Some(serde_json::to_value(summary)?),
                    message,
                )
                .await?
                .ok_or_else(denied)?;
            }
            FinishPackJob::Prepared { receipt }
                if job.payload.trigger_reason == PREPARATION_TRIGGER
                    && !receipt.scope.is_object_backed() =>
            {
                if !RuntimePreparationRepository::finish_with(
                    tx,
                    id,
                    job.worker.ok_or_else(denied)?,
                    manager.instance,
                    &receipt,
                )
                .await?
                {
                    return Err(denied());
                }
            }
            FinishPackJob::Failed => {
                PackInstallRepository::finish_running_with(
                    &mut **tx,
                    id,
                    PackInstallStatus::Failed,
                    None,
                    None,
                    Some("The manager could not run the pack job".into()),
                )
                .await?
                .ok_or_else(denied)?;
            }
            _ => return Err(denied()),
        }
        Self::receipt(tx, id, &digest).await?;
        ManagerRuntimeRepository::check_deadline(tx, manager, expires.min(job.deadline)).await?;
        Ok(())
    }

    pub async fn authorize_finish(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        expires: DateTime<Utc>,
        id: Id,
        claim: Uuid,
    ) -> Result<()> {
        let (job, install) = Self::scoped(tx, manager, id, claim).await?;
        if job.finish_digest.is_some() {
            ManagerRuntimeRepository::check_deadline(tx, manager, expires).await
        } else {
            Self::check_running(tx, manager, expires, &job, &install).await
        }
    }

    async fn receipt(conn: &mut PgConnection, id: Id, digest: &str) -> Result<()> {
        sqlx::query("UPDATE manager_pack_job SET finish_digest=$2 WHERE install=$1")
            .bind(id)
            .bind(digest)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Run from the executor. Redelivery uses stored placement inputs and a
    /// new attempt. An old process cannot claim a redelivered offer by ID.
    pub async fn recover(pool: &PgPool) -> Result<Option<PackTestRequestedPayload>> {
        let mut tx = pool.begin().await?;
        let id: Option<Id> = sqlx::query_scalar(
            "SELECT i.id FROM pack_install i JOIN manager_pack_job j ON j.install=i.id
             LEFT JOIN manager_session s ON s.id=j.manager_session
             LEFT JOIN manager_action_worker m ON m.worker=j.worker
             WHERE j.finish_digest IS NULL AND (
               (j.worker IS NULL AND i.status='pending' AND j.deadline <= clock_timestamp()) OR
               (j.worker IS NOT NULL AND i.status='running' AND (
                 j.deadline <= clock_timestamp() OR s.closed_at IS NOT NULL OR s.expires_at <= clock_timestamp()
                 OR m.manager_session IS DISTINCT FROM j.manager_session OR (m.draining AND j.claim IS NULL))))
             ORDER BY j.deadline, i.id LIMIT 1 FOR UPDATE OF i SKIP LOCKED",
        ).fetch_optional(&mut *tx).await?;
        let Some(id) = id else {
            return Ok(None);
        };
        let job: ManagerPackJob = sqlx::query_as(&format!(
            "SELECT {JOB_COLUMNS} FROM manager_pack_job WHERE install=$1 FOR UPDATE"
        ))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let within_budget: bool = sqlx::query_scalar("SELECT started_at + make_interval(secs=>$2) > clock_timestamp() FROM pack_install WHERE id=$1")
            .bind(id).bind(PACK_INSTALL_ACTIVE_TTL_SECS as f64).fetch_one(&mut *tx).await?
            && (job.payload.trigger_reason != PREPARATION_TRIGGER
                || RuntimePreparationRepository::dispatch_job(&mut *tx, id).await?.is_some());
        if job.worker.is_some() {
            let expired: bool = sqlx::query_scalar(
                "SELECT j.deadline <= clock_timestamp() OR s.closed_at IS NOT NULL OR s.expires_at <= clock_timestamp()
                     OR m.manager_session IS DISTINCT FROM j.manager_session OR (m.draining AND j.claim IS NULL)
                 FROM manager_pack_job j JOIN manager_session s ON s.id=j.manager_session
                 LEFT JOIN manager_action_worker m ON m.worker=j.worker WHERE j.install=$1",
            ).bind(id).fetch_one(&mut *tx).await?;
            if !expired || job.finish_digest.is_some() {
                return Ok(None);
            }
            let retry = job.attempt < MAX_ATTEMPTS && within_budget;
            if !retry {
                PackInstallRepository::finish_running_with(
                    &mut *tx,
                    id,
                    PackInstallStatus::Failed,
                    None,
                    None,
                    Some("Pack job expired before a manager could complete it".into()),
                )
                .await?;
                tx.commit().await?;
                return Ok(None);
            }
            sqlx::query("UPDATE pack_install SET status='pending', assigned_worker_id=NULL, candidate_access_token_hash=NULL,updated_at=clock_timestamp() WHERE id=$1")
                .bind(id).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM pack_candidate_context WHERE install_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE pack_runtime_preparation_attempt SET builder_incarnation=NULL,builder_scope=NULL WHERE install=$1 AND prepared IS NULL")
                .bind(id).execute(&mut *tx).await?;
        } else if !within_budget {
            sqlx::query("UPDATE pack_install SET status='failed',error_message='Pack job expired before a manager accepted it',
                         updated_at=clock_timestamp(),finished_at=clock_timestamp() WHERE id=$1 AND status='pending'")
                .bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        sqlx::query("UPDATE manager_pack_job SET worker=NULL,manager_session=NULL,instance=NULL,claim=NULL,
                     attempt=attempt+CASE WHEN worker IS NULL THEN 0 ELSE 1 END,deadline=clock_timestamp()+interval '5 seconds'
                     WHERE install=$1")
            .bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some(job.payload.0))
    }
}
