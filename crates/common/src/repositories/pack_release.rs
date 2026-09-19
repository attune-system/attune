//! Repository operations for immutable pack releases.

use crate::models::PackRelease;
use crate::{Error, Result};
use sqlx::{Executor, Postgres, Transaction};

const PACK_RELEASE_COLUMNS: &str =
    "id, pack, pack_ref, version, digest, object_key, provider_version, content_path, archive_size, manifest, created, inactive_since";

#[derive(Debug)]
pub struct CreatePackReleaseInput {
    pub pack: i64,
    pub pack_ref: String,
    pub version: String,
    pub digest: String,
    pub object_key: String,
    pub provider_version: String,
    pub content_path: String,
    pub archive_size: i64,
    pub manifest: serde_json::Value,
}

pub struct PackReleaseRepository;

impl PackReleaseRepository {
    pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<PackRelease>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {PACK_RELEASE_COLUMNS} FROM pack_release WHERE id = $1");
        Ok(sqlx::query_as(&query)
            .bind(id)
            .fetch_optional(executor)
            .await?)
    }

    pub async fn list_by_pack<'e, E>(executor: E, pack: i64) -> Result<Vec<PackRelease>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {PACK_RELEASE_COLUMNS} FROM pack_release WHERE pack = $1 ORDER BY created, id"
        );
        Ok(sqlx::query_as(&query)
            .bind(pack)
            .fetch_all(executor)
            .await?)
    }

    pub async fn find_active_by_pack_ref<'e, E>(
        executor: E,
        pack_ref: &str,
    ) -> Result<Option<PackRelease>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM pack_release r JOIN pack p ON p.active_release = r.id WHERE p.ref = $1",
            PACK_RELEASE_COLUMNS
                .split(", ")
                .map(|column| format!("r.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(sqlx::query_as(&query)
            .bind(pack_ref)
            .fetch_optional(executor)
            .await?)
    }

    pub async fn find_by_pack_version(
        transaction: &mut Transaction<'_, Postgres>,
        pack: i64,
        version: &str,
    ) -> Result<Option<PackRelease>> {
        let query = format!(
            "SELECT {PACK_RELEASE_COLUMNS} FROM pack_release WHERE pack = $1 AND version = $2"
        );
        Ok(sqlx::query_as(&query)
            .bind(pack)
            .bind(version)
            .fetch_optional(&mut **transaction)
            .await?)
    }

    /// Insert a release, or return the existing byte-identical release.
    pub async fn create_or_get(
        transaction: &mut Transaction<'_, Postgres>,
        input: CreatePackReleaseInput,
    ) -> Result<PackRelease> {
        if let Some(existing) =
            Self::find_by_pack_version(transaction, input.pack, &input.version).await?
        {
            if existing.digest != input.digest {
                return Err(Error::already_exists(
                    "Pack release",
                    "version",
                    format!(
                        "{}@{} has digest {}",
                        input.pack_ref, input.version, existing.digest
                    ),
                ));
            }
            let query = format!(
                "UPDATE pack_release SET object_key = $2, provider_version = $3, archive_size = $4 \
                 WHERE id = $1 RETURNING {PACK_RELEASE_COLUMNS}"
            );
            return sqlx::query_as(&query)
                .bind(existing.id)
                .bind(input.object_key)
                .bind(input.provider_version)
                .bind(input.archive_size)
                .fetch_one(&mut **transaction)
                .await
                .map_err(Into::into);
        }

        let query = format!(
            "INSERT INTO pack_release (pack, pack_ref, version, digest, object_key, provider_version, content_path, archive_size, manifest) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {PACK_RELEASE_COLUMNS}"
        );
        sqlx::query_as(&query)
            .bind(input.pack)
            .bind(input.pack_ref)
            .bind(input.version)
            .bind(input.digest)
            .bind(input.object_key)
            .bind(input.provider_version)
            .bind(input.content_path)
            .bind(input.archive_size)
            .bind(input.manifest)
            .fetch_one(&mut **transaction)
            .await
            .map_err(Into::into)
    }

    pub async fn activate(
        transaction: &mut Transaction<'_, Postgres>,
        pack: i64,
        release: i64,
    ) -> Result<()> {
        // Lock the pack before releases or components. Component inserts take a
        // conflicting SHARE lock while selecting their managed release.
        sqlx::query("SELECT id FROM pack WHERE id = $1 FOR NO KEY UPDATE")
            .bind(pack)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or_else(|| Error::not_found("pack", "id", pack.to_string()))?;
        sqlx::query(
            "UPDATE pack_release SET inactive_since = NOW() WHERE pack = $1 \
             AND id = (SELECT active_release FROM pack WHERE id = $1) AND id <> $2",
        )
        .bind(pack)
        .bind(release)
        .execute(&mut **transaction)
        .await?;
        let result = sqlx::query(
            "WITH activated AS ( \
                 UPDATE pack_release SET inactive_since = NULL WHERE id = $2 AND pack = $1 RETURNING content_path \
             ) UPDATE pack SET active_release = $2, storage_path = (SELECT content_path FROM activated), updated = NOW() \
             WHERE id = $1 AND EXISTS (SELECT 1 FROM activated)",
        )
        .bind(pack)
        .bind(release)
        .execute(&mut **transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(Error::not_found("pack release", "id", release.to_string()));
        }
        Ok(())
    }

    pub async fn activate_projected(
        transaction: &mut Transaction<'_, Postgres>,
        pack: i64,
        release: i64,
        projections: &super::component_lifecycle::PackProjectionIds,
    ) -> Result<()> {
        Self::activate(transaction, pack, release).await?;
        super::component_lifecycle::ComponentLifecycleRepository::stamp_release(
            transaction,
            pack,
            release,
            projections,
        )
        .await?;
        super::executable_snapshot::ExecutableSnapshotRepository::capture_present(
            transaction,
            release,
            &projections.actions,
            &projections.sensors,
        )
        .await
    }
}
