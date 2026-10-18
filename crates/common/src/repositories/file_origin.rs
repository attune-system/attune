//! Exact version-bound process file sources and encrypted body completion.
use super::runtime_read::{RuntimeReadOrigin, RuntimeReadRepository};
use crate::{runtime_file_body::FileBodyManifest, value_schema::digest, Error, Result};
use sqlx::{FromRow, PgConnection, PgPool};

#[derive(Clone, FromRow)]
pub struct FileOrigin {
    pub artifact_version: i64,
    pub origin_read: i64,
    pub admission: i64,
    pub execution: i64,
    pub attempt: uuid::Uuid,
    pub sponsor: i64,
    pub max_plaintext_bytes: i64,
    pub source_digest: String,
    pub receipts: serde_json::Value,
}

#[derive(Clone, FromRow)]
pub struct FileBody {
    pub artifact_version: i64,
    pub origin_read: i64,
    pub plaintext_bytes: i64,
    pub plaintext_sha256: String,
    pub ciphertext_bytes: i64,
    pub ciphertext_sha256: String,
    pub authenticated_manifest: String,
}

impl FileBody {
    pub fn manifest(&self) -> FileBodyManifest {
        FileBodyManifest {
            encoding: crate::runtime_file_body::ENCODING.into(),
            version: self.artifact_version,
            origin_read: self.origin_read,
            plaintext_bytes: self.plaintext_bytes as u64,
            plaintext_sha256: self.plaintext_sha256.clone(),
            ciphertext_bytes: self.ciphertext_bytes as u64,
            ciphertext_sha256: self.ciphertext_sha256.clone(),
        }
    }
}

pub struct FileOriginRepository;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedSourceRow {
    entity_type: String,
    entity_id: i64,
    json_path: String,
    source_kind: String,
    source_ref: Option<String>,
}

fn denied() -> Error {
    Error::PermissionDenied("Exact file source or body proof is unavailable".into())
}

impl FileOriginRepository {
    /// Authenticate the reservation's ORIGINAL source sites for a publication
    /// effect. The pending destination is not itself an acquired readable file.
    /// This returns no authority; callers still prepare/authorize every source
    /// through the canonical evaluator before capture or destination locks.
    pub async fn retained_sources(
        conn: &mut PgConnection,
        retained: &FileOrigin,
    ) -> Result<Vec<super::execution_secret_value::SecretProvenanceRow>> {
        let original = RuntimeReadRepository::find(&mut *conn, retained.origin_read).await?;
        let bound: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_runtime_read_origin WHERE id=$1 AND admission=$2 AND file_sink_version=$3)")
            .bind(retained.origin_read).bind(retained.admission).bind(retained.artifact_version)
            .fetch_one(&mut *conn).await?;
        if !bound {
            return Err(denied());
        }
        Self::decode_retained_sources(retained, &original)
    }

    fn decode_retained_sources(
        retained: &FileOrigin,
        original: &RuntimeReadOrigin,
    ) -> Result<Vec<super::execution_secret_value::SecretProvenanceRow>> {
        if !matches!(&original.target,super::runtime_read::RuntimeReadTarget::ArtifactVersion {version,execution:Some(execution),..}
            if *version==retained.artifact_version && *execution==retained.execution)
            || digest(&serde_json::json!({"origin":original,"receipts":retained.receipts}))
                .map_err(|_| denied())?
                != retained.source_digest
            || serde_json::to_vec(&retained.receipts)?.len() > super::runtime_read::MAX_READ_BYTES
        {
            return Err(denied());
        }
        let rows: Vec<RetainedSourceRow> =
            serde_json::from_value(retained.receipts.clone()).map_err(|_| denied())?;
        if rows.len() > 256 {
            return Err(denied());
        }
        Ok(rows
            .into_iter()
            .map(|row| super::execution_secret_value::SecretProvenanceRow {
                entity_type: row.entity_type,
                entity_id: row.entity_id,
                json_path: row.json_path,
                source_kind: row.source_kind,
                source_ref: row.source_ref,
            })
            .collect())
    }

    /// A retained plaintext buffer is not permission to serve a body after its
    /// exact version enters cleanup or its evidence disappears.
    pub async fn require_ready_body(
        pool: &PgPool,
        version: &crate::models::ArtifactVersion,
        body: &FileBody,
    ) -> Result<()> {
        let mut conn = pool.acquire().await?;
        Self::require_ready_body_on(&mut conn, version, body).await
    }
    pub async fn require_ready_body_on(
        conn: &mut PgConnection,
        version: &crate::models::ArtifactVersion,
        body: &FileBody,
    ) -> Result<()> {
        super::file_evidence_retention::FileEvidenceRetentionRepository::require_retained(
            conn, version.id,
        )
        .await?;
        let ready:Option<i64>=sqlx::query_scalar("SELECT v.id FROM artifact_version v JOIN artifact_file_origin o ON o.artifact_version=v.id JOIN artifact_file_body b ON b.artifact_version=v.id WHERE v.id=$1 AND v.body_state='ready' AND o.origin_read=$2 AND b.origin_read=$2 AND v.object_key IS NOT DISTINCT FROM $3 AND v.provider_version IS NOT DISTINCT FROM $4 AND v.size_bytes=$5 AND v.sha256=$6 AND b.ciphertext_bytes=v.size_bytes AND b.ciphertext_sha256=v.sha256 AND b.authenticated_manifest=$7 FOR SHARE OF v")
            .bind(version.id).bind(body.origin_read).bind(&version.object_key).bind(&version.provider_version)
            .bind(body.ciphertext_bytes).bind(&body.ciphertext_sha256).bind(&body.authenticated_manifest).fetch_optional(conn).await?;
        if ready.is_none() {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn pending_body(
        conn: &mut PgConnection,
        version: i64,
    ) -> Result<Option<(FileBody, Vec<u8>)>> {
        let proof:Option<FileBody> = sqlx::query_as("SELECT artifact_version,origin_read,plaintext_bytes,plaintext_sha256,ciphertext_bytes,ciphertext_sha256,authenticated_manifest FROM artifact_file_body WHERE artifact_version=$1")
            .bind(version).fetch_optional(&mut *conn).await?;
        let Some(proof) = proof else {
            return Ok(None);
        };
        let bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT ciphertext FROM artifact_file_upload WHERE artifact_version=$1",
        )
        .bind(version)
        .fetch_optional(conn)
        .await?
        .ok_or_else(denied)?;
        Ok(Some((proof, bytes)))
    }

    pub async fn stage(
        conn: &mut PgConnection,
        manifest: &FileBodyManifest,
        authenticated: &str,
        ciphertext: &[u8],
    ) -> Result<()> {
        if ciphertext.len() as u64 != manifest.ciphertext_bytes
            || crate::runtime_file_body::sha256(ciphertext) != manifest.ciphertext_sha256
        {
            return Err(denied());
        }
        Self::complete(conn, manifest, authenticated).await?;
        sqlx::query(
            "INSERT INTO artifact_file_upload (artifact_version,ciphertext) VALUES ($1,$2)",
        )
        .bind(manifest.version)
        .bind(ciphertext)
        .execute(conn)
        .await?;
        Ok(())
    }

    pub async fn finish(conn: &mut PgConnection, version: i64) -> Result<()> {
        sqlx::query("DELETE FROM artifact_file_upload WHERE artifact_version=$1")
            .bind(version)
            .execute(conn)
            .await?;
        Ok(())
    }
    pub async fn find(conn: &mut PgConnection, version: i64) -> Result<Option<FileOrigin>> {
        let origin=sqlx::query_as("SELECT artifact_version,origin_read,admission,execution,attempt,sponsor,max_plaintext_bytes,source_digest,receipts FROM artifact_file_origin WHERE artifact_version=$1")
            .bind(version).fetch_optional(&mut *conn).await?;
        if origin.is_some() {
            super::file_evidence_retention::FileEvidenceRetentionRepository::require_retained(
                conn, version,
            )
            .await?;
        }
        Ok(origin)
    }

    pub async fn body(pool: &PgPool, version: i64) -> Result<Option<FileBody>> {
        Ok(sqlx::query_as("SELECT artifact_version,origin_read,plaintext_bytes,plaintext_sha256,ciphertext_bytes,ciphertext_sha256,authenticated_manifest FROM artifact_file_body WHERE artifact_version=$1")
            .bind(version).fetch_optional(pool).await?)
    }

    /// Worker completion may inspect a closed registry; it never reopens acquisition.
    pub async fn lock_capture(conn: &mut PgConnection, admission: i64) -> Result<()> {
        sqlx::query("INSERT INTO execution_runtime_read_fence (admission) VALUES ($1) ON CONFLICT DO NOTHING").bind(admission).execute(&mut *conn).await?;
        sqlx::query(
            "SELECT admission FROM execution_runtime_read_fence WHERE admission=$1 FOR UPDATE",
        )
        .bind(admission)
        .fetch_one(conn)
        .await?;
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Bind physical file custody to every execution credential and provenance field"
    )]
    pub async fn reserve(
        conn: &mut PgConnection,
        version: i64,
        admission: i64,
        execution: i64,
        attempt: uuid::Uuid,
        sponsor: i64,
        origin: &RuntimeReadOrigin,
        receipts: &[super::execution_secret_value::SecretProvenanceRow],
    ) -> Result<()> {
        origin.validate()?;
        if !matches!(&origin.target,super::runtime_read::RuntimeReadTarget::ArtifactVersion {version:v,execution:Some(e),..}
            if *v==version && *e==execution)
        {
            return Err(denied());
        }
        let pending:bool=sqlx::query_scalar("SELECT v.execution IS NOT DISTINCT FROM $2 AND v.body_state='pending' AND a.classification<>'runtime_log' AND NOT EXISTS (SELECT 1 FROM log_stream WHERE artifact_version=$1) FROM artifact_version v JOIN artifact a ON a.id=v.artifact WHERE v.id=$1")
            .bind(version).bind(execution).fetch_optional(&mut *conn).await?.unwrap_or(false);
        if !pending {
            return Err(denied());
        }
        let body = serde_json::to_value(origin)?;
        let hash = digest(&body).map_err(|_| denied())?;
        let id: i64 = sqlx::query_scalar("INSERT INTO execution_runtime_read_origin (admission,origin_digest,origin,file_sink_version) VALUES ($1,$2,$3,$4) RETURNING id")
            .bind(admission).bind(&hash).bind(body).bind(version).fetch_one(&mut *conn).await?;
        let receipts = Self::receipts(receipts)?;
        let source_digest = digest(&serde_json::json!({"origin":origin,"receipts":receipts}))
            .map_err(|_| denied())?;
        let inserted=sqlx::query("INSERT INTO artifact_file_origin (artifact_version,origin_read,admission,execution,attempt,sponsor,source_digest,receipts,max_plaintext_bytes) SELECT $1,$2,$3,$4,$5,$6,$7,$8,max_bytes FROM worker_delivery_permit WHERE artifact_version=$1")
            .bind(version).bind(id).bind(admission).bind(execution).bind(attempt).bind(sponsor).bind(source_digest).bind(receipts).execute(&mut *conn).await?.rows_affected();
        if inserted != 1 {
            return Err(denied());
        }
        sqlx::query("INSERT INTO artifact_file_staging_guard(file_path,artifact_version) SELECT file_path,id FROM artifact_version WHERE id=$1 AND file_path IS NOT NULL")
            .bind(version).execute(&mut *conn).await?;
        super::file_evidence_retention::FileEvidenceRetentionRepository::retain(conn, version, id)
            .await?;
        Ok(())
    }

    pub async fn guarded_path(conn: &mut PgConnection, path: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM artifact_file_staging_guard WHERE file_path=$1)",
        )
        .bind(path)
        .fetch_one(conn)
        .await?)
    }

    pub async fn orphaned_staging(pool: &PgPool, limit: i64) -> Result<Vec<(String, i64)>> {
        Ok(sqlx::query_as("SELECT g.file_path,g.artifact_version FROM artifact_file_staging_guard g WHERE NOT EXISTS(SELECT 1 FROM artifact_version v WHERE v.id=g.artifact_version OR v.file_path=g.file_path) ORDER BY g.artifact_version,g.file_path LIMIT $1")
            .bind(limit.max(1)).fetch_all(pool).await?)
    }

    /// Only the native cleaner calls this AFTER physical staging deletion.
    pub async fn release_staging(pool: &PgPool, path: &str, version: i64) -> Result<()> {
        let mut tx = pool.begin().await?;
        super::file_evidence_retention::FileEvidenceRetentionRepository::staging_removed(
            &mut tx, path, version,
        )
        .await?;
        sqlx::query("DELETE FROM artifact_file_staging_guard g WHERE file_path=$1 AND artifact_version=$2 AND NOT EXISTS(SELECT 1 FROM artifact_version v WHERE v.id=g.artifact_version OR v.file_path=g.file_path)")
            .bind(path).bind(version).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn require_sources(
        conn: &mut PgConnection,
        retained: &FileOrigin,
        fresh: &RuntimeReadOrigin,
        receipts: &[super::execution_secret_value::SecretProvenanceRow],
    ) -> Result<()> {
        fresh.validate()?;
        let receipts = Self::receipts(receipts)?;
        if receipts != retained.receipts
            || digest(&serde_json::json!({"origin":fresh,"receipts":receipts}))
                .map_err(|_| denied())?
                != retained.source_digest
            || RuntimeReadRepository::find(conn, retained.origin_read).await? != *fresh
        {
            return Err(denied());
        }
        Ok(())
    }

    fn receipts(
        rows: &[super::execution_secret_value::SecretProvenanceRow],
    ) -> Result<serde_json::Value> {
        let value=serde_json::Value::Array(rows.iter().map(|row|serde_json::json!({
            "entity_type":row.entity_type,"entity_id":row.entity_id,"json_path":row.json_path,
            "source_kind":row.source_kind,"source_ref":row.source_ref,
        })).collect());
        if rows.len() > 256
            || serde_json::to_vec(&value)?.len() > super::runtime_read::MAX_READ_BYTES
        {
            return Err(denied());
        }
        Ok(value)
    }

    pub async fn complete(
        conn: &mut PgConnection,
        manifest: &FileBodyManifest,
        authenticated: &str,
    ) -> Result<()> {
        let origin = Self::find(conn, manifest.version)
            .await?
            .ok_or_else(denied)?;
        if origin.origin_read != manifest.origin_read
            || manifest.plaintext_bytes > origin.max_plaintext_bytes as u64
            || manifest.encoding != crate::runtime_file_body::ENCODING
        {
            return Err(denied());
        }
        sqlx::query("INSERT INTO artifact_file_body (artifact_version,origin_read,plaintext_bytes,plaintext_sha256,ciphertext_bytes,ciphertext_sha256,authenticated_manifest) VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (artifact_version) DO NOTHING")
            .bind(manifest.version).bind(manifest.origin_read).bind(i64::try_from(manifest.plaintext_bytes).map_err(|_|denied())?)
            .bind(&manifest.plaintext_sha256).bind(i64::try_from(manifest.ciphertext_bytes).map_err(|_|denied())?)
            .bind(&manifest.ciphertext_sha256).bind(authenticated).execute(&mut *conn).await?;
        let matches: bool = sqlx::query_scalar("SELECT origin_read=$2 AND plaintext_bytes=$3 AND plaintext_sha256=$4 AND ciphertext_bytes=$5 AND ciphertext_sha256=$6 AND authenticated_manifest=$7 FROM artifact_file_body WHERE artifact_version=$1")
            .bind(manifest.version).bind(manifest.origin_read).bind(manifest.plaintext_bytes as i64).bind(&manifest.plaintext_sha256)
            .bind(manifest.ciphertext_bytes as i64).bind(&manifest.ciphertext_sha256).bind(authenticated).fetch_one(conn).await?;
        if !matches {
            return Err(denied());
        }
        Ok(())
    }
}

#[cfg(test)]
mod retained_source_tests {
    use super::*;
    use serde_json::json;

    fn reservation() -> (FileOrigin, RuntimeReadOrigin) {
        let original: RuntimeReadOrigin = serde_json::from_value(json!({
            "target":{"kind":"artifact_version","artifact":1,"version":2,"execution":3,"owner":{}},
            "provenance":[],"parents":[],
        }))
        .unwrap();
        let receipts = json!([
            {"entity_type":"execution_config","entity_id":3,"json_path":"/input","source_kind":"provenance",
             "source_ref":"original caller receipt at its exact input site"},
            {"entity_type":"execution_runtime_read","entity_id":9,"json_path":"","source_kind":"provenance",
             "source_ref":"actual acquired read reference, never the pending sink"}
        ]);
        let source_digest = digest(&json!({"origin":original,"receipts":receipts})).unwrap();
        (
            FileOrigin {
                artifact_version: 2,
                origin_read: 8,
                admission: 7,
                execution: 3,
                attempt: uuid::Uuid::nil(),
                sponsor: 4,
                max_plaintext_bytes: 1024,
                source_digest,
                receipts,
            },
            original,
        )
    }

    #[test]
    fn publication_sources_keep_original_sites_and_acquired_reads_not_the_destination() {
        let (retained, original) = reservation();
        let rows = FileOriginRepository::decode_retained_sources(&retained, &original).unwrap();
        assert_eq!(
            FileOriginRepository::receipts(&rows).unwrap(),
            retained.receipts
        );
        assert_eq!(rows[0].entity_type, "execution_config");
        assert_eq!(rows[0].entity_id, 3);
        assert_eq!(rows[0].json_path, "/input");
        assert_eq!(rows[1].entity_id, 9);
        assert_ne!(rows[1].entity_id, retained.origin_read);
    }

    #[test]
    fn changed_sources_or_destination_bindings_cannot_be_rebased() {
        let (mut retained, original) = reservation();
        retained.receipts[0]["entity_id"] = json!(300);
        assert!(FileOriginRepository::decode_retained_sources(&retained, &original).is_err());
        let (mut retained, original) = reservation();
        retained.execution = 300;
        assert!(FileOriginRepository::decode_retained_sources(&retained, &original).is_err());
        let (mut retained, original) = reservation();
        retained.artifact_version = 200;
        assert!(FileOriginRepository::decode_retained_sources(&retained, &original).is_err());
    }

    #[test]
    fn frozen_rows_must_have_the_exact_bounded_reservation_shape() {
        let (mut retained, original) = reservation();
        retained.receipts[0]["unexpected"] = json!(true);
        retained.source_digest =
            digest(&json!({"origin":original,"receipts":retained.receipts})).unwrap();
        assert!(FileOriginRepository::decode_retained_sources(&retained, &original).is_err());
        let (mut retained, original) = reservation();
        retained.receipts = json!(vec![retained.receipts[0].clone(); 257]);
        retained.source_digest =
            digest(&json!({"origin":original,"receipts":retained.receipts})).unwrap();
        assert!(FileOriginRepository::decode_retained_sources(&retained, &original).is_err());
    }
}
