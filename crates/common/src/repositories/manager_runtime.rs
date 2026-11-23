use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgConnection, Postgres, Transaction};

use crate::{
    manager_control::{
        ManagerEnrollmentGrant, ManagerRuntimeCatalog, ManagerRuntimeReport, RuntimeReportReceipt,
    },
    models::{Id, Worker, WorkerRole},
    runtime_detection::normalize_runtime_name,
    runtime_lifecycle::validate_version_mode,
    Error, Result,
};

use super::{
    manager_assignment::ManagerAssignmentRepository, manager_session::ManagerSessionRecord,
    runtime::WORKER_SELECT_COLUMNS, runtime_version::RuntimeVersionRepository, List,
    RuntimeRepository, WorkerRepository,
};

pub struct ManagerRuntimeRepository;

impl ManagerRuntimeRepository {
    pub async fn lock_revision(conn: &mut PgConnection) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT revision FROM manager_runtime_catalog WHERE singleton FOR SHARE",
        )
        .fetch_one(conn)
        .await?)
    }

    pub async fn catalog(
        conn: &mut PgConnection,
        revision: i64,
        object_preparation: bool,
    ) -> Result<ManagerRuntimeCatalog> {
        let mut runtimes = RuntimeRepository::list(&mut *conn).await?;
        let mut versions = RuntimeVersionRepository::list(&mut *conn).await?;
        // Discovery only needs executable and verification metadata.
        for runtime in &mut runtimes {
            runtime.installation = None;
            runtime.installers = Value::Null;
            runtime.detection_config = Value::Null;
        }
        for version in &mut versions {
            version.meta = Value::Null;
        }
        Ok(ManagerRuntimeCatalog {
            revision,
            runtimes,
            versions,
            object_preparation,
        })
    }

    pub async fn publish(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        bearer_expires_at: DateTime<Utc>,
        catalog: &ManagerRuntimeCatalog,
        report: &ManagerRuntimeReport,
        grant: &ManagerEnrollmentGrant,
    ) -> Result<RuntimeReportReceipt> {
        let mut capabilities = project(catalog, report)?;
        let mut labels = grant.labels.clone();
        labels.insert("managed_pool".into(), manager.pool.clone());
        capabilities["labels"] = json!(labels);
        capabilities["taints"] = json!(grant.taints);
        if manager.role == WorkerRole::Action && !report.pack_jobs_only {
            capabilities["delegated_execution_authority_v1"] = json!(true);
            capabilities["log_delivery_instance"] = json!(manager.instance);
            capabilities[crate::action_cli::WORKER_CAPABILITY] = json!(true);
        }
        let digest = crate::runtime_file_body::sha256(&serde_json::to_vec(report)?);
        let previous: Option<(Id, i64, String)> = sqlx::query_as(
            "SELECT worker, sequence, digest FROM manager_runtime_report WHERE manager_session=$1",
        )
        .bind(manager.id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some((worker, sequence, previous_digest)) = previous {
            if sequence >= report.sequence {
                if sequence != report.sequence || previous_digest != digest {
                    return Err(Error::InvalidState(
                        "Runtime report sequence has already advanced".into(),
                    ));
                }
                Self::check_deadline(tx, manager, bearer_expires_at).await?;
                return Ok(RuntimeReportReceipt {
                    worker,
                    revision: report.revision,
                    sequence,
                });
            }
        }

        let role = match manager.role {
            WorkerRole::Action => "action",
            WorkerRole::Sensor => "sensor",
        };
        let name = format!("manager/{role}/{}/{}", manager.pool, manager.name);
        let worker = sqlx::query_as::<_, Worker>(&format!(
            "INSERT INTO worker (name, worker_type, worker_role, status, capabilities, meta, last_heartbeat)
             VALUES ($1, 'container', $2, 'active',
                     $3, jsonb_build_object('registration_incarnation', $4::text), clock_timestamp())
             ON CONFLICT (name) DO UPDATE SET status='active', last_heartbeat=clock_timestamp(),
                updated=clock_timestamp(),
                capabilities=(COALESCE(worker.capabilities,'{{}}'::jsonb) - 'runtime_preparation_filter' - 'log_delivery_instance' - 'delegated_execution_authority_v1') || $3,
                meta=COALESCE(worker.meta,'{{}}'::jsonb) || EXCLUDED.meta
             WHERE worker.worker_role=EXCLUDED.worker_role
             AND EXISTS (SELECT 1 FROM manager_runtime_report r WHERE r.worker=worker.id)
             RETURNING {WORKER_SELECT_COLUMNS}"
        ))
        .bind(name)
        .bind(manager.role)
        .bind(capabilities)
        .bind(manager.instance.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| Error::PermissionDenied("Worker name belongs to a different registration".into()))?;

        if manager.role == WorkerRole::Action {
            ManagerAssignmentRepository::bind_action_worker(tx, worker.id, manager).await?;
        }
        sqlx::query(
            "INSERT INTO manager_runtime_report (worker, manager_session, revision, sequence, digest)
             VALUES ($1,$2,$3,$4,$5)
             ON CONFLICT (worker) DO UPDATE SET manager_session=EXCLUDED.manager_session,
                 revision=EXCLUDED.revision, sequence=EXCLUDED.sequence, digest=EXCLUDED.digest",
        )
        .bind(worker.id)
        .bind(manager.id)
        .bind(report.revision)
        .bind(report.sequence)
        .bind(digest)
        .execute(&mut **tx)
        .await?;
        Self::check_deadline(tx, manager, bearer_expires_at).await?;
        Ok(RuntimeReportReceipt {
            worker: worker.id,
            revision: report.revision,
            sequence: report.sequence,
        })
    }

    pub async fn check_deadline(
        conn: &mut PgConnection,
        manager: &ManagerSessionRecord,
        bearer_expires_at: DateTime<Utc>,
    ) -> Result<()> {
        let live: bool = sqlx::query_scalar("SELECT $1 > clock_timestamp()")
            .bind(manager.expires_at.min(bearer_expires_at))
            .fetch_one(conn)
            .await?;
        if !live {
            return Err(Error::PermissionDenied("Manager session expired".into()));
        }
        Ok(())
    }

    pub async fn heartbeat(conn: &mut PgConnection, manager: &ManagerSessionRecord) -> Result<()> {
        sqlx::query(
            "UPDATE worker w SET last_heartbeat=clock_timestamp() FROM manager_runtime_report r
             WHERE w.id=r.worker AND r.manager_session=$1
                 AND w.meta->>'registration_incarnation'=$2::uuid::text",
        )
        .bind(manager.id)
        .bind(manager.instance)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Hold exact placement evidence until the scheduler commits its offer.
    /// Existing in-flight work remains valid when the catalog changes.
    pub async fn retain_placement(conn: &mut PgConnection, selected: &Worker) -> Result<bool> {
        let managed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM manager_runtime_report WHERE worker=$1)",
        )
        .bind(selected.id)
        .fetch_one(&mut *conn)
        .await?;
        if !managed {
            return Ok(true);
        }
        Self::lock_revision(conn).await?;
        let Some(row) = WorkerRepository::find_registration_for_share(conn, selected.id).await?
        else {
            return Ok(false);
        };
        let current: bool = sqlx::query_scalar("SELECT manager_runtime_is_current($1)")
            .bind(selected.id)
            .fetch_one(conn)
            .await?;
        Ok(current
            && !row.cordoned
            && row.status == selected.status
            && row.capabilities == selected.capabilities
            && row
                .meta
                .as_ref()
                .and_then(|meta| meta.get("registration_incarnation"))
                == selected
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("registration_incarnation")))
    }

    /// Retain the current process binding for narrowly scoped API operations.
    pub async fn lock_worker(
        conn: &mut PgConnection,
        manager: &ManagerSessionRecord,
    ) -> Result<Id> {
        sqlx::query_scalar(
            "SELECT w.id FROM worker w JOIN manager_runtime_report r ON r.worker=w.id
             WHERE r.manager_session=$1 AND w.worker_role=$2
               AND w.meta->>'registration_incarnation'=$3::uuid::text FOR SHARE OF w",
        )
        .bind(manager.id)
        .bind(manager.role)
        .bind(manager.instance)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| Error::PermissionDenied("Manager worker registration is stale".into()))
    }
}

fn project(catalog: &ManagerRuntimeCatalog, report: &ManagerRuntimeReport) -> Result<Value> {
    let invalid = || Error::validation("Invalid runtime capability evidence");
    if report.sequence <= 0
        || report.recipe_digests.len() > 1024
        || report.runtime_filter.as_ref().is_some_and(|filter| {
            filter.len() > 128
                || filter.iter().any(|name| {
                    name.is_empty() || name.len() > 128 || name.chars().any(char::is_control)
                })
        })
        || report.recipe_digests.iter().any(|digest| {
            digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        || report
            .preparation_scope
            .as_ref()
            .is_some_and(|scope| !scope.valid_for_report(catalog.object_preparation))
    {
        return Err(invalid());
    }

    let runtimes: BTreeMap<_, _> = catalog.runtimes.iter().map(|row| (row.id, row)).collect();
    let versions: BTreeMap<_, _> = catalog.versions.iter().map(|row| (row.id, row)).collect();
    let parents_with_versions: BTreeSet<_> =
        catalog.versions.iter().map(|row| row.runtime).collect();
    let mut names = BTreeSet::new();
    let mut toolchains = BTreeMap::new();
    let mut legacy_versions = BTreeMap::<String, BTreeSet<String>>::new();
    for (id, probe) in &report.runtimes {
        let runtime = runtimes.get(id).ok_or_else(invalid)?;
        if !probe.default_available && probe.versions.is_empty() {
            return Err(invalid());
        }
        let aliases: BTreeSet<_> = runtime
            .aliases
            .iter()
            .chain([&runtime.name])
            .map(|name| normalize_runtime_name(name))
            .collect();
        let mut verified = BTreeSet::new();
        for version_id in &probe.versions {
            let version = versions
                .get(version_id)
                .filter(|version| version.runtime == *id)
                .ok_or_else(invalid)?;
            validate_version_mode(&runtime.execution_config, &version.execution_config)
                .map_err(|_| invalid())?;
            if let Some(recipe) = version.parsed_execution_config().lifecycle {
                let digest = recipe.digest().map_err(|_| invalid())?;
                if !report.recipe_digests.contains(&digest) {
                    return Err(invalid());
                }
            }
            verified.insert(&version.version);
        }
        if runtime.execution_config.get("lifecycle").is_some() {
            let recipe = runtime
                .parsed_execution_config()
                .lifecycle
                .ok_or_else(invalid)?;
            let digest = recipe.digest().map_err(|_| invalid())?;
            if probe.recipe_digest.as_ref() != Some(&digest)
                || !report.recipe_digests.contains(&digest)
            {
                return Err(invalid());
            }
            if let Some(version) = &probe.primary_version {
                if !probe.default_available
                    || version.is_empty()
                    || version.len() > 128
                    || version.chars().any(char::is_control)
                {
                    return Err(invalid());
                }
                if !parents_with_versions.contains(id) {
                    verified.insert(version);
                }
            } else if probe.default_available {
                return Err(invalid());
            }
            toolchains.insert(
                &runtime.r#ref,
                json!({"digest":digest,"versions":verified,"names":aliases}),
            );
        } else {
            if probe.recipe_digest.is_some() || probe.primary_version.is_some() {
                return Err(invalid());
            }
            if !verified.is_empty() {
                for alias in &aliases {
                    legacy_versions
                        .entry(alias.clone())
                        .or_default()
                        .extend(verified.iter().map(|version| (*version).clone()));
                }
            }
        }
        names.extend(aliases);
    }
    let mut capabilities = json!({
        "runtimes":names, "runtime_toolchains":toolchains, "runtime_versions":legacy_versions,
        "runtime_recipe_digests":report.recipe_digests, "runtime_preparation_scope":report.preparation_scope,
        "pack_jobs_only":report.pack_jobs_only
    });
    if let Some(filter) = &report.runtime_filter {
        capabilities["runtime_preparation_filter"] = json!(filter);
    }
    Ok(capabilities)
}
