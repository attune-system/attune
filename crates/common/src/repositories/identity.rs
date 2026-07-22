//! Identity and permission repository for database operations

use crate::models::{identity::*, Id, JsonDict};
use crate::Result;
use sqlx::{Executor, PgPool, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, FindByRef, List, Repository, Update};

pub struct IdentityRepository;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteIdentityOutcome {
    Deleted,
    NotFound,
    CacheCleanupPending { namespaces: u64 },
}

impl Repository for IdentityRepository {
    type Entity = Identity;
    fn table_name() -> &'static str {
        "identities"
    }
}

#[derive(Debug, Clone)]
pub struct CreateIdentityInput {
    pub login: String,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub attributes: JsonDict,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateIdentityInput {
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub attributes: Option<JsonDict>,
    pub frozen: Option<bool>,
}

#[async_trait::async_trait]
impl FindById for IdentityRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated FROM identity WHERE id = $1"
        ).bind(id).fetch_optional(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for IdentityRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated FROM identity ORDER BY login ASC"
        ).fetch_all(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for IdentityRepository {
    type CreateInput = CreateIdentityInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "INSERT INTO identity (login, display_name, password_hash, attributes) VALUES ($1, $2, $3, $4) RETURNING id, login, display_name, password_hash, attributes, frozen, created, updated"
        )
        .bind(&input.login)
        .bind(&input.display_name)
        .bind(&input.password_hash)
        .bind(&input.attributes)
        .fetch_one(executor)
        .await
        .map_err(|e| {
            // Convert unique constraint violation to AlreadyExists error
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.is_unique_violation() {
                    return crate::Error::already_exists("Identity", "login", &input.login);
                }
            }
            e.into()
        })
    }
}

#[async_trait::async_trait]
impl Update for IdentityRepository {
    type UpdateInput = UpdateIdentityInput;
    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query
        let mut query = QueryBuilder::new("UPDATE identity SET ");
        let mut has_updates = false;

        if let Some(display_name) = &input.display_name {
            query.push("display_name = ").push_bind(display_name);
            has_updates = true;
        }
        if let Some(password_hash) = &input.password_hash {
            if has_updates {
                query.push(", ");
            }
            query.push("password_hash = ").push_bind(password_hash);
            has_updates = true;
        }
        if let Some(attributes) = &input.attributes {
            if has_updates {
                query.push(", ");
            }
            query.push("attributes = ").push_bind(attributes);
            has_updates = true;
        }
        if let Some(frozen) = input.frozen {
            if has_updates {
                query.push(", ");
            }
            query.push("frozen = ").push_bind(frozen);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(
            " RETURNING id, login, display_name, password_hash, attributes, frozen, created, updated",
        );

        query
            .build_query_as::<Identity>()
            .fetch_one(executor)
            .await
            .map_err(|e| {
                // Convert RowNotFound to NotFound error
                if matches!(e, sqlx::Error::RowNotFound) {
                    return crate::Error::not_found("identity", "id", id.to_string());
                }
                e.into()
            })
    }
}

#[async_trait::async_trait]
impl Delete for IdentityRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM identity WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl IdentityRepository {
    /// Deletes an identity when no cache namespace still references it.
    ///
    /// Identity-owned namespaces must drain through the cache retention
    /// lifecycle. The first delete request tombstones every blocking namespace
    /// atomically and leaves the identity in place; a later request can delete
    /// the identity after retention removes those namespaces.
    pub async fn delete_with_cache_lifecycle(
        pool: &PgPool,
        id: Id,
    ) -> Result<DeleteIdentityOutcome> {
        let mut tx = pool.begin().await?;
        let identity_exists =
            sqlx::query_scalar::<_, Id>("SELECT id FROM identity WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .is_some();

        if !identity_exists {
            tx.rollback().await?;
            return Ok(DeleteIdentityOutcome::NotFound);
        }

        let namespace_ids = sqlx::query_scalar::<_, Id>(
            "SELECT id FROM cache_namespace \
             WHERE owner_identity = $1 ORDER BY id FOR UPDATE",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;

        if !namespace_ids.is_empty() {
            sqlx::query(
                "UPDATE cache_namespace \
                 SET tombstoned_at = COALESCE(tombstoned_at, NOW()), \
                     active_generation = NULL, \
                     tombstone_reason = COALESCE(tombstone_reason, 'owning identity deleted') \
                 WHERE id = ANY($1::BIGINT[])",
            )
            .bind(&namespace_ids)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "UPDATE cache_generation \
                 SET state = 'failed', failed = NOW(), \
                     failure_reason = 'owning identity deleted' \
                 WHERE namespace = ANY($1::BIGINT[]) AND state IN ('staging', 'ready')",
            )
            .bind(&namespace_ids)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "UPDATE cache_generation \
                 SET state = 'retired', retired = NOW(), readable_until = NOW() \
                 WHERE namespace = ANY($1::BIGINT[]) AND state = 'active'",
            )
            .bind(&namespace_ids)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            return Ok(DeleteIdentityOutcome::CacheCleanupPending {
                namespaces: namespace_ids.len() as u64,
            });
        }

        let deleted = Self::delete(&mut *tx, id).await?;
        tx.commit().await?;
        Ok(if deleted {
            DeleteIdentityOutcome::Deleted
        } else {
            DeleteIdentityOutcome::NotFound
        })
    }

    pub async fn update_display_name<'e, E>(
        executor: E,
        id: Id,
        display_name: Option<&str>,
    ) -> Result<Identity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "UPDATE identity
                SET display_name = $2,
                    updated = NOW()
              WHERE id = $1
              RETURNING id, login, display_name, password_hash, attributes, frozen, created, updated",
        )
        .bind(id)
        .bind(display_name)
        .fetch_one(executor)
        .await
        .map_err(|e| {
            if matches!(e, sqlx::Error::RowNotFound) {
                return crate::Error::not_found("identity", "id", id.to_string());
            }
            e.into()
        })
    }

    pub async fn find_by_login<'e, E>(executor: E, login: &str) -> Result<Option<Identity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated FROM identity WHERE login = $1"
        ).bind(login).fetch_optional(executor).await.map_err(Into::into)
    }

    /// Strict OIDC identity lookup keyed by `(issuer, sub, client_id)`.
    ///
    /// The `client_id` is included in the match key as defense-in-depth so that
    /// two configured OIDC clients sharing an issuer cannot collide on a single
    /// identity record. Rows that predate the introduction of `client_id`
    /// (where the JSON attribute is missing/NULL) will not be returned by this
    /// query — use [`find_legacy_oidc_subject`](Self::find_legacy_oidc_subject)
    /// for the upgrade fallback.
    pub async fn find_by_oidc_subject<'e, E>(
        executor: E,
        issuer: &str,
        subject: &str,
        client_id: &str,
    ) -> Result<Option<Identity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated
             FROM identity
             WHERE attributes->'oidc'->>'issuer' = $1
               AND attributes->'oidc'->>'sub' = $2
               AND attributes->'oidc'->>'client_id' = $3",
        )
        .bind(issuer)
        .bind(subject)
        .bind(client_id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    /// Legacy OIDC identity lookup for rows that do not yet have a
    /// `client_id` recorded in `attributes->'oidc'`. Matches only on
    /// `(issuer, sub)` AND requires `client_id` to be NULL/absent. Used by the
    /// graceful upgrade path in the OIDC callback so that pre-existing users
    /// can still log in and have their record stamped with a `client_id`.
    pub async fn find_legacy_oidc_subject<'e, E>(
        executor: E,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<Identity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated
             FROM identity
             WHERE attributes->'oidc'->>'issuer' = $1
               AND attributes->'oidc'->>'sub' = $2
               AND attributes->'oidc'->>'client_id' IS NULL",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_ldap_dn<'e, E>(
        executor: E,
        server_url: &str,
        dn: &str,
    ) -> Result<Option<Identity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated
             FROM identity
             WHERE attributes->'ldap'->>'server_url' = $1
               AND attributes->'ldap'->>'dn' = $2",
        )
        .bind(server_url)
        .bind(dn)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    /// Lookup any OIDC identity row for the given `(issuer, sub)`, regardless
    /// of whether `client_id` is recorded. Used as the ultimate fallback in
    /// the race-safe upsert path when a unique-constraint violation indicates
    /// a concurrent insert won.
    pub async fn find_oidc_by_issuer_sub<'e, E>(
        executor: E,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<Identity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated
             FROM identity
             WHERE attributes->'oidc'->>'issuer' = $1
               AND attributes->'oidc'->>'sub' = $2",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    /// Race-safe OIDC identity upsert.
    ///
    /// Resolves an OIDC identity from `(issuer, sub, client_id)` and creates
    /// or updates the underlying row atomically. Three concurrent logins for
    /// the same user can race here; the implementation guarantees that
    /// exactly one `identity` row exists for `(issuer, sub)` afterwards.
    ///
    /// The race-safety strategy combines:
    /// 1. A per-attempt transaction so each lookup→write step is atomic.
    /// 2. A guarded UPDATE on the legacy upgrade path
    ///    (`WHERE … client_id IS NULL`) so only one concurrent caller can
    ///    stamp the legacy row with a `client_id`. The loser sees 0 rows
    ///    affected and retries from the top.
    /// 3. The `uq_identity_oidc_issuer_sub` partial unique index, which
    ///    catches the (rare) case where two concurrent callers both fall
    ///    through to INSERT. The loser receives a 23505 violation, rolls
    ///    back, and reads the winner's row from the database.
    ///
    /// Roles are deliberately *not* synchronized here — that lives in the
    /// caller (the API service) since it depends on infrastructure outside
    /// the common crate.
    pub async fn upsert_oidc_identity(pool: &PgPool, input: OidcUpsertInput) -> Result<Identity> {
        // Cap retries to avoid pathological loops if the database is in an
        // unexpected state. In practice 1–2 iterations are sufficient.
        const MAX_ATTEMPTS: u32 = 5;

        for _attempt in 0..MAX_ATTEMPTS {
            let mut tx = pool.begin().await?;

            // Stage 1: strict (issuer, sub, client_id) match — the canonical
            // lookup for any row already stamped with our client_id.
            if let Some(existing) =
                Self::find_by_oidc_subject(&mut *tx, &input.issuer, &input.sub, &input.client_id)
                    .await?
            {
                let updated = Self::update_oidc_row(
                    &mut *tx,
                    existing.id,
                    input.display_name.as_deref(),
                    &input.attributes,
                )
                .await?;
                tx.commit().await?;
                return Ok(updated);
            }

            // Stage 2: legacy fallback — adopt a row that pre-dates the
            // client_id-aware code path. The guarded UPDATE ensures only one
            // concurrent caller can win the upgrade.
            if let Some(legacy) =
                Self::find_legacy_oidc_subject(&mut *tx, &input.issuer, &input.sub).await?
            {
                let result = sqlx::query(
                    "UPDATE identity
                       SET display_name = COALESCE($2, display_name),
                           attributes = $3,
                           updated = NOW()
                     WHERE id = $1
                       AND attributes->'oidc'->>'client_id' IS NULL",
                )
                .bind(legacy.id)
                .bind(input.display_name.as_deref())
                .bind(&input.attributes)
                .execute(&mut *tx)
                .await?;

                if result.rows_affected() == 1 {
                    let updated = sqlx::query_as::<_, Identity>(
                        "SELECT id, login, display_name, password_hash, attributes, frozen, created, updated
                         FROM identity WHERE id = $1",
                    )
                    .bind(legacy.id)
                    .fetch_one(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    return Ok(updated);
                }

                // 0 rows affected → another caller already upgraded the
                // legacy row. Roll back and retry: the strict lookup at the
                // top of the next attempt will succeed (if our client_id
                // matches) or fall through to INSERT (which the unique
                // index will resolve).
                tx.rollback().await?;
                continue;
            }

            // Stage 3: INSERT. Login uniqueness is handled by the existing
            // `identity_login_key` constraint; OIDC (issuer, sub) uniqueness
            // is handled by `uq_identity_oidc_issuer_sub`.
            let login = if Self::find_by_login(&mut *tx, &input.desired_login)
                .await?
                .is_some()
            {
                input.fallback_login.clone()
            } else {
                input.desired_login.clone()
            };

            let insert_result = sqlx::query_as::<_, Identity>(
                "INSERT INTO identity (login, display_name, password_hash, attributes)
                 VALUES ($1, $2, NULL, $3)
                 RETURNING id, login, display_name, password_hash, attributes, frozen, created, updated",
            )
            .bind(&login)
            .bind(input.display_name.as_deref())
            .bind(&input.attributes)
            .fetch_one(&mut *tx)
            .await;

            match insert_result {
                Ok(identity) => {
                    tx.commit().await?;
                    return Ok(identity);
                }
                Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
                    // Roll back so we can read freshly-committed state.
                    tx.rollback().await?;

                    let constraint = db_err.constraint().map(|c| c.to_string());
                    if constraint.as_deref() == Some("uq_identity_oidc_issuer_sub") {
                        // Concurrent OIDC insert won. Read the winning row
                        // by (issuer, sub) and return it as-is. We do *not*
                        // overwrite the winner's attributes here because
                        // their client_id may legitimately differ from
                        // ours; if it matches, a subsequent login will take
                        // the strict-match path and update normally.
                        if let Some(winner) =
                            Self::find_oidc_by_issuer_sub(pool, &input.issuer, &input.sub).await?
                        {
                            return Ok(winner);
                        }
                        // Extremely unlikely: row vanished between the
                        // 23505 violation and our follow-up read. Retry.
                        continue;
                    }

                    // Some other unique violation (e.g. login race after
                    // we picked a fallback). Fail loudly — this should not
                    // happen during normal OIDC operation.
                    return Err(sqlx::Error::Database(db_err).into());
                }
                Err(other) => {
                    tx.rollback().await?;
                    return Err(other.into());
                }
            }
        }

        Err(crate::Error::internal(
            "OIDC identity upsert exceeded retry limit",
        ))
    }

    async fn update_oidc_row<'e, E>(
        executor: E,
        id: Id,
        display_name: Option<&str>,
        attributes: &JsonDict,
    ) -> Result<Identity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Identity>(
            "UPDATE identity
                SET display_name = COALESCE($2, display_name),
                    attributes = $3,
                    updated = NOW()
              WHERE id = $1
              RETURNING id, login, display_name, password_hash, attributes, frozen, created, updated",
        )
        .bind(id)
        .bind(display_name)
        .bind(attributes)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }
}

/// Input bundle for the race-safe OIDC identity upsert.
#[derive(Debug, Clone)]
pub struct OidcUpsertInput {
    pub issuer: String,
    pub sub: String,
    pub client_id: String,
    /// Preferred login for newly-created identities (typically email).
    pub desired_login: String,
    /// Fallback login used when `desired_login` collides with an existing
    /// identity (typically `oidc:<issuer>:<sub>`-derived).
    pub fallback_login: String,
    pub display_name: Option<String>,
    /// Full attributes payload — must contain an `"oidc"` object with at
    /// least `issuer`, `sub`, and `client_id` for the partial unique index
    /// to match correctly.
    pub attributes: JsonDict,
}

// Permission Set Repository
pub struct PermissionSetRepository;

impl Repository for PermissionSetRepository {
    type Entity = PermissionSet;
    fn table_name() -> &'static str {
        "permission_set"
    }
}

#[derive(Debug, Clone)]
pub struct CreatePermissionSetInput {
    pub r#ref: String,
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub grants: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct UpdatePermissionSetInput {
    pub label: Option<String>,
    pub description: Option<String>,
    pub grants: Option<serde_json::Value>,
}

#[async_trait::async_trait]
impl FindById for PermissionSetRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSet>(
            "SELECT id, ref, pack, pack_ref, label, description, grants, created, updated FROM permission_set WHERE id = $1"
        ).bind(id).fetch_optional(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl FindByRef for PermissionSetRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSet>(
            "SELECT id, ref, pack, pack_ref, label, description, grants, created, updated FROM permission_set WHERE ref = $1"
        )
        .bind(ref_str)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for PermissionSetRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSet>(
            "SELECT id, ref, pack, pack_ref, label, description, grants, created, updated FROM permission_set ORDER BY ref ASC"
        ).fetch_all(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for PermissionSetRepository {
    type CreateInput = CreatePermissionSetInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSet>(
            "INSERT INTO permission_set (ref, pack, pack_ref, label, description, grants) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, ref, pack, pack_ref, label, description, grants, created, updated"
        ).bind(&input.r#ref).bind(input.pack).bind(&input.pack_ref).bind(&input.label).bind(&input.description).bind(&input.grants).fetch_one(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for PermissionSetRepository {
    type UpdateInput = UpdatePermissionSetInput;
    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query
        let mut query = QueryBuilder::new("UPDATE permission_set SET ");
        let mut has_updates = false;

        if let Some(label) = &input.label {
            query.push("label = ").push_bind(label);
            has_updates = true;
        }
        if let Some(description) = &input.description {
            if has_updates {
                query.push(", ");
            }
            query.push("description = ").push_bind(description);
            has_updates = true;
        }
        if let Some(grants) = &input.grants {
            if has_updates {
                query.push(", ");
            }
            query.push("grants = ").push_bind(grants);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(
            " RETURNING id, ref, pack, pack_ref, label, description, grants, created, updated",
        );

        query
            .build_query_as::<PermissionSet>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for PermissionSetRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM permission_set WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl PermissionSetRepository {
    pub async fn find_by_refs<'e, E>(executor: E, refs: &[String]) -> Result<Vec<PermissionSet>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if refs.is_empty() {
            return Ok(Vec::new());
        }

        sqlx::query_as::<_, PermissionSet>(
            "SELECT id, ref, pack, pack_ref, label, description, grants, created, updated
             FROM permission_set
             WHERE ref = ANY($1)
             ORDER BY ref ASC",
        )
        .bind(refs)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_identity<'e, E>(executor: E, identity_id: Id) -> Result<Vec<PermissionSet>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSet>(
            "SELECT ps.id, ps.ref, ps.pack, ps.pack_ref, ps.label, ps.description, ps.grants, ps.created, ps.updated
             FROM permission_set ps
             INNER JOIN permission_assignment pa ON pa.permset = ps.id
             WHERE pa.identity = $1
             ORDER BY ps.ref ASC",
        )
        .bind(identity_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_roles<'e, E>(executor: E, roles: &[String]) -> Result<Vec<PermissionSet>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if roles.is_empty() {
            return Ok(Vec::new());
        }

        sqlx::query_as::<_, PermissionSet>(
            "SELECT DISTINCT ps.id, ps.ref, ps.pack, ps.pack_ref, ps.label, ps.description, ps.grants, ps.created, ps.updated
             FROM permission_set ps
             INNER JOIN permission_set_role_assignment psra ON psra.permset = ps.id
             WHERE psra.role = ANY($1)
             ORDER BY ps.ref ASC",
        )
        .bind(roles)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    /// Delete permission sets belonging to a pack whose refs are NOT in the given set.
    ///
    /// Used during pack reinstallation to clean up permission sets that were
    /// removed from the pack's metadata. Associated permission assignments are
    /// cascade-deleted by the FK constraint.
    pub async fn delete_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = if keep_refs.is_empty() {
            sqlx::query("DELETE FROM permission_set WHERE pack = $1")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query("DELETE FROM permission_set WHERE pack = $1 AND ref != ALL($2)")
                .bind(pack_id)
                .bind(keep_refs)
                .execute(executor)
                .await?
        };

        Ok(result.rows_affected())
    }
}

// Permission Assignment Repository
pub struct PermissionAssignmentRepository;

impl Repository for PermissionAssignmentRepository {
    type Entity = PermissionAssignment;
    fn table_name() -> &'static str {
        "permission_assignment"
    }
}

#[derive(Debug, Clone)]
pub struct CreatePermissionAssignmentInput {
    pub identity: Id,
    pub permset: Id,
}

#[async_trait::async_trait]
impl FindById for PermissionAssignmentRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionAssignment>(
            "SELECT id, identity, permset, created FROM permission_assignment WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for PermissionAssignmentRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionAssignment>(
            "SELECT id, identity, permset, created FROM permission_assignment ORDER BY created DESC"
        ).fetch_all(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for PermissionAssignmentRepository {
    type CreateInput = CreatePermissionAssignmentInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionAssignment>(
            "INSERT INTO permission_assignment (identity, permset) VALUES ($1, $2) RETURNING id, identity, permset, created"
        ).bind(input.identity).bind(input.permset).fetch_one(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for PermissionAssignmentRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM permission_assignment WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl PermissionAssignmentRepository {
    pub async fn find_by_identity<'e, E>(
        executor: E,
        identity_id: Id,
    ) -> Result<Vec<PermissionAssignment>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionAssignment>(
            "SELECT id, identity, permset, created FROM permission_assignment WHERE identity = $1",
        )
        .bind(identity_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }
}

pub struct IdentityRoleAssignmentRepository;

impl Repository for IdentityRoleAssignmentRepository {
    type Entity = IdentityRoleAssignment;
    fn table_name() -> &'static str {
        "identity_role_assignment"
    }
}

#[derive(Debug, Clone)]
pub struct CreateIdentityRoleAssignmentInput {
    pub identity: Id,
    pub role: String,
    pub source: String,
    pub managed: bool,
}

#[async_trait::async_trait]
impl FindById for IdentityRoleAssignmentRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IdentityRoleAssignment>(
            "SELECT id, identity, role, source, managed, created, updated FROM identity_role_assignment WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for IdentityRoleAssignmentRepository {
    type CreateInput = CreateIdentityRoleAssignmentInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IdentityRoleAssignment>(
            "INSERT INTO identity_role_assignment (identity, role, source, managed)
             VALUES ($1, $2, $3, $4)
             RETURNING id, identity, role, source, managed, created, updated",
        )
        .bind(input.identity)
        .bind(&input.role)
        .bind(&input.source)
        .bind(input.managed)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for IdentityRoleAssignmentRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM identity_role_assignment WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl IdentityRoleAssignmentRepository {
    pub async fn find_by_identity<'e, E>(
        executor: E,
        identity_id: Id,
    ) -> Result<Vec<IdentityRoleAssignment>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IdentityRoleAssignment>(
            "SELECT id, identity, role, source, managed, created, updated
             FROM identity_role_assignment
             WHERE identity = $1
             ORDER BY role ASC",
        )
        .bind(identity_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_role_names_by_identity<'e, E>(
        executor: E,
        identity_id: Id,
    ) -> Result<Vec<String>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_scalar::<_, String>(
            "SELECT role FROM identity_role_assignment WHERE identity = $1 ORDER BY role ASC",
        )
        .bind(identity_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn replace_managed_roles<'e, E>(
        executor: E,
        identity_id: Id,
        source: &str,
        roles: &[String],
    ) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        sqlx::query(
            "DELETE FROM identity_role_assignment WHERE identity = $1 AND source = $2 AND managed = true",
        )
        .bind(identity_id)
        .bind(source)
        .execute(executor)
        .await?;

        for role in roles {
            sqlx::query(
                "INSERT INTO identity_role_assignment (identity, role, source, managed)
                 VALUES ($1, $2, $3, true)
                 ON CONFLICT (identity, role) DO UPDATE
                 SET source = EXCLUDED.source,
                     managed = EXCLUDED.managed,
                     updated = NOW()",
            )
            .bind(identity_id)
            .bind(role)
            .bind(source)
            .execute(executor)
            .await?;
        }

        Ok(())
    }
}

pub struct PermissionSetRoleAssignmentRepository;

impl Repository for PermissionSetRoleAssignmentRepository {
    type Entity = PermissionSetRoleAssignment;
    fn table_name() -> &'static str {
        "permission_set_role_assignment"
    }
}

#[derive(Debug, Clone)]
pub struct CreatePermissionSetRoleAssignmentInput {
    pub permset: Id,
    pub role: String,
}

#[async_trait::async_trait]
impl FindById for PermissionSetRoleAssignmentRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSetRoleAssignment>(
            "SELECT id, permset, role, created FROM permission_set_role_assignment WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for PermissionSetRoleAssignmentRepository {
    type CreateInput = CreatePermissionSetRoleAssignmentInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSetRoleAssignment>(
            "INSERT INTO permission_set_role_assignment (permset, role)
             VALUES ($1, $2)
             RETURNING id, permset, role, created",
        )
        .bind(input.permset)
        .bind(&input.role)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for PermissionSetRoleAssignmentRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM permission_set_role_assignment WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl PermissionSetRoleAssignmentRepository {
    pub async fn find_by_permission_set<'e, E>(
        executor: E,
        permset_id: Id,
    ) -> Result<Vec<PermissionSetRoleAssignment>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, PermissionSetRoleAssignment>(
            "SELECT id, permset, role, created
             FROM permission_set_role_assignment
             WHERE permset = $1
             ORDER BY role ASC",
        )
        .bind(permset_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }
}
