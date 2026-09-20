use sqlx::{Executor, PgConnection, Postgres};

use crate::models::identity::ExternalIdentityMapping;
use crate::models::Id;
use crate::{Error, Result};

pub const SELECT_COLUMNS: &str = "id, integration_identity, mapped_identity, provider, tenant, external_subject, created_by, created, updated";

pub struct ExternalIdentityMappingRepository;

#[derive(Debug, Clone)]
pub struct ResolvedExternalIdentity {
    pub mapping: ExternalIdentityMapping,
    pub identity: MappedExternalIdentity,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MappedExternalIdentity {
    pub id: Id,
    pub login: String,
}

#[derive(sqlx::FromRow)]
struct ResolvedExternalIdentityRow {
    mapping_id: Id,
    integration_identity: Id,
    mapped_identity: Id,
    provider: String,
    tenant: String,
    external_subject: String,
    created_by: Option<Id>,
    mapping_created: chrono::DateTime<chrono::Utc>,
    mapping_updated: chrono::DateTime<chrono::Utc>,
    identity_login: String,
}

#[derive(Debug, Clone)]
pub struct CreateExternalIdentityMappingInput {
    pub mapped_identity: Id,
    pub provider: String,
    pub tenant: String,
    pub external_subject: String,
    pub created_by: Option<Id>,
}

#[derive(Debug, Clone)]
pub struct UpdateExternalIdentityMappingInput {
    pub mapped_identity: Id,
    pub provider: String,
    pub tenant: String,
    pub external_subject: String,
}

#[derive(Debug)]
struct NormalizedExternalIdentityKey {
    provider: String,
    tenant: String,
    external_subject: String,
}

impl ExternalIdentityMappingRepository {
    pub async fn create<'e, E>(
        executor: E,
        integration_identity: Id,
        input: CreateExternalIdentityMappingInput,
    ) -> Result<ExternalIdentityMapping>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let key = normalize_key(&input.provider, &input.tenant, &input.external_subject)?;
        sqlx::query_as::<_, ExternalIdentityMapping>(&format!(
            "INSERT INTO external_identity_mapping \
             (integration_identity, mapped_identity, provider, tenant, external_subject, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {SELECT_COLUMNS}"
        ))
        .bind(integration_identity)
        .bind(input.mapped_identity)
        .bind(key.provider)
        .bind(key.tenant)
        .bind(key.external_subject)
        .bind(input.created_by)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_id<'e, E>(
        executor: E,
        integration_identity: Id,
        id: Id,
    ) -> Result<Option<ExternalIdentityMapping>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, ExternalIdentityMapping>(&format!(
            "SELECT {SELECT_COLUMNS} FROM external_identity_mapping \
             WHERE integration_identity = $1 AND id = $2"
        ))
        .bind(integration_identity)
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_id_for_update(
        conn: &mut PgConnection,
        integration_identity: Id,
        id: Id,
    ) -> Result<Option<ExternalIdentityMapping>> {
        sqlx::query_as::<_, ExternalIdentityMapping>(&format!(
            "SELECT {SELECT_COLUMNS} FROM external_identity_mapping \
             WHERE integration_identity = $1 AND id = $2 FOR UPDATE"
        ))
        .bind(integration_identity)
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(Into::into)
    }

    pub async fn list<'e, E>(
        executor: E,
        integration_identity: Id,
    ) -> Result<Vec<ExternalIdentityMapping>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, ExternalIdentityMapping>(&format!(
            "SELECT {SELECT_COLUMNS} FROM external_identity_mapping \
             WHERE integration_identity = $1 ORDER BY id"
        ))
        .bind(integration_identity)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn list_page<'e, E>(
        executor: E,
        integration_identity: Id,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ExternalIdentityMapping>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, ExternalIdentityMapping>(&format!(
            "SELECT {SELECT_COLUMNS} FROM external_identity_mapping \
             WHERE integration_identity = $1 ORDER BY id LIMIT $2 OFFSET $3"
        ))
        .bind(integration_identity)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn update<'e, E>(
        executor: E,
        integration_identity: Id,
        id: Id,
        input: UpdateExternalIdentityMappingInput,
    ) -> Result<ExternalIdentityMapping>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let key = normalize_key(&input.provider, &input.tenant, &input.external_subject)?;
        sqlx::query_as::<_, ExternalIdentityMapping>(&format!(
            "UPDATE external_identity_mapping SET mapped_identity = $3, provider = $4, \
             tenant = $5, external_subject = $6, updated = NOW() \
             WHERE integration_identity = $1 AND id = $2 RETURNING {SELECT_COLUMNS}"
        ))
        .bind(integration_identity)
        .bind(id)
        .bind(input.mapped_identity)
        .bind(key.provider)
        .bind(key.tenant)
        .bind(key.external_subject)
        .fetch_one(executor)
        .await
        .map_err(|error| match error {
            sqlx::Error::RowNotFound => {
                Error::not_found("external_identity_mapping", "id", id.to_string())
            }
            error => error.into(),
        })
    }

    pub async fn delete<'e, E>(executor: E, integration_identity: Id, id: Id) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "DELETE FROM external_identity_mapping WHERE integration_identity = $1 AND id = $2",
        )
        .bind(integration_identity)
        .bind(id)
        .execute(executor)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Resolves and locks an exact mapping and its active mapped identity.
    ///
    /// Call this inside the transaction that consumes the resolved identity so
    /// the mapping cannot be deleted and the identity cannot be frozen before
    /// the caller finishes its authorization decision.
    pub async fn resolve_exact_for_share<'e, E>(
        executor: E,
        integration_identity: Id,
        provider: &str,
        tenant: &str,
        external_subject: &str,
    ) -> Result<Option<MappedExternalIdentity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let key = normalize_key(provider, tenant, external_subject)?;
        sqlx::query_as::<_, MappedExternalIdentity>(
            "SELECT i.id, i.login \
             FROM external_identity_mapping m \
             JOIN identity i ON i.id = m.mapped_identity \
             WHERE m.integration_identity = $1 AND m.provider = $2 \
               AND m.tenant = $3 AND m.external_subject = $4 AND NOT i.frozen \
              FOR SHARE OF m, i",
        )
        .bind(integration_identity)
        .bind(key.provider)
        .bind(key.tenant)
        .bind(key.external_subject)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    /// Resolves the exact normalized key and locks both the mapping and its
    /// active mapped identity for a provider response transaction.
    pub async fn resolve_exact_with_mapping_for_share(
        conn: &mut PgConnection,
        integration_identity: Id,
        provider: &str,
        tenant: &str,
        external_subject: &str,
    ) -> Result<Option<ResolvedExternalIdentity>> {
        let key = normalize_key(provider, tenant, external_subject)?;
        let row = sqlx::query_as::<_, ResolvedExternalIdentityRow>(
            "SELECT m.id AS mapping_id, m.integration_identity, m.mapped_identity, \
                    m.provider, m.tenant, m.external_subject, m.created_by, \
                    m.created AS mapping_created, m.updated AS mapping_updated, \
                     i.login AS identity_login \
             FROM external_identity_mapping m \
             JOIN identity i ON i.id = m.mapped_identity \
             WHERE m.integration_identity = $1 AND m.provider = $2 \
               AND m.tenant = $3 AND m.external_subject = $4 AND NOT i.frozen \
             FOR SHARE OF m, i",
        )
        .bind(integration_identity)
        .bind(key.provider)
        .bind(key.tenant)
        .bind(key.external_subject)
        .fetch_optional(conn)
        .await?;

        Ok(row.map(|row| ResolvedExternalIdentity {
            mapping: ExternalIdentityMapping {
                id: row.mapping_id,
                integration_identity: row.integration_identity,
                mapped_identity: row.mapped_identity,
                provider: row.provider,
                tenant: row.tenant,
                external_subject: row.external_subject,
                created_by: row.created_by,
                created: row.mapping_created,
                updated: row.mapping_updated,
            },
            identity: MappedExternalIdentity {
                id: row.mapped_identity,
                login: row.identity_login,
            },
        }))
    }
}

fn normalize_key(
    provider: &str,
    tenant: &str,
    external_subject: &str,
) -> Result<NormalizedExternalIdentityKey> {
    let provider = provider.trim().to_ascii_lowercase();
    let tenant = tenant.trim().to_string();
    let external_subject = external_subject.trim().to_string();

    let mut provider_chars = provider.chars();
    if provider.len() > 64
        || !provider_chars
            .next()
            .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        || !provider_chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        })
    {
        return Err(Error::validation(
            "provider must be a lowercase token of at most 64 characters",
        ));
    }

    validate_external_component("tenant", &tenant)?;
    validate_external_component("external_subject", &external_subject)?;

    Ok(NormalizedExternalIdentityKey {
        provider,
        tenant,
        external_subject,
    })
}

fn validate_external_component(name: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.chars().count() > 255 {
        return Err(Error::validation(format!(
            "{name} must be nonempty and at most 255 characters"
        )));
    }
    Ok(())
}
