//! Repository for API-managed pack registry indices.

use crate::models::PackRegistryIndex;
use crate::{Error, Result};
use sqlx::{Executor, Postgres};

use super::{Delete, FindById, List, Repository};

pub struct PackRegistryIndexRepository;

impl Repository for PackRegistryIndexRepository {
    type Entity = PackRegistryIndex;

    fn table_name() -> &'static str {
        "pack_registry_index"
    }
}

#[derive(Debug, Clone)]
pub struct CreatePackRegistryIndexInput {
    pub name: Option<String>,
    pub url: String,
    pub position: Option<i32>,
    pub enabled: bool,
    pub headers: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct UpdatePackRegistryIndexInput {
    pub name: Option<Option<String>>,
    pub url: Option<String>,
    pub position: Option<i32>,
    pub enabled: Option<bool>,
    pub headers: Option<serde_json::Value>,
}

const COLUMNS: &str = "id, name, url, position, enabled, is_standard, headers, created, updated";

#[async_trait::async_trait]
impl FindById for PackRegistryIndexRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM pack_registry_index WHERE id = $1", COLUMNS);
        sqlx::query_as::<_, PackRegistryIndex>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for PackRegistryIndexRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM pack_registry_index ORDER BY position ASC, id ASC",
            COLUMNS
        );
        sqlx::query_as::<_, PackRegistryIndex>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for PackRegistryIndexRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM pack_registry_index WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl PackRegistryIndexRepository {
    /// Replace encrypted headers only when the row still contains the value that
    /// the caller originally read. This prevents legacy write-on-read migration
    /// from overwriting a concurrent credential rotation.
    pub async fn compare_and_set_headers<'e, E>(
        executor: E,
        id: i64,
        expected: &serde_json::Value,
        replacement: serde_json::Value,
    ) -> Result<Option<PackRegistryIndex>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        validate_encrypted_headers(&replacement)?;
        let query = format!(
            r#"
            UPDATE pack_registry_index
            SET headers = $3
            WHERE id = $1 AND headers = $2
            RETURNING {}
            "#,
            COLUMNS
        );
        sqlx::query_as::<_, PackRegistryIndex>(&query)
            .bind(id)
            .bind(expected)
            .bind(replacement)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn create<'e, E>(
        executor: E,
        input: CreatePackRegistryIndexInput,
    ) -> Result<PackRegistryIndex>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let url = normalize_url(&input.url)?;
        validate_encrypted_headers(&input.headers)?;
        if matches!(input.position, Some(position) if position < 0) {
            return Err(Error::validation("Index position must be non-negative"));
        }

        let query = format!(
            r#"
            INSERT INTO pack_registry_index (name, url, position, enabled, headers)
            VALUES (
                $1,
                $2,
                COALESCE(
                    $3,
                    (
                        SELECT LEAST(
                            COALESCE(MAX(position)::bigint + 1, 0),
                            2147483647
                        )::integer
                        FROM pack_registry_index
                    )
                ),
                $4,
                $5
            )
            RETURNING {}
            "#,
            COLUMNS
        );
        sqlx::query_as::<_, PackRegistryIndex>(&query)
            .bind(input.name)
            .bind(url)
            .bind(input.position)
            .bind(input.enabled)
            .bind(input.headers)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn update<'e, E>(
        executor: E,
        id: i64,
        input: UpdatePackRegistryIndexInput,
    ) -> Result<PackRegistryIndex>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let url = input.url.as_deref().map(normalize_url).transpose()?;
        if let Some(headers) = &input.headers {
            validate_encrypted_headers(headers)?;
        }
        if matches!(input.position, Some(position) if position < 0) {
            return Err(Error::validation("Index position must be non-negative"));
        }

        let update_name = input.name.is_some();
        let name = input.name.flatten();

        let query = format!(
            r#"
            UPDATE pack_registry_index
            SET name = CASE WHEN $2 THEN $3 ELSE name END,
                url = COALESCE($4, url),
                position = COALESCE($5, position),
                enabled = COALESCE($6, enabled),
                headers = COALESCE($7, headers)
            WHERE id = $1
            RETURNING {}
            "#,
            COLUMNS
        );
        sqlx::query_as::<_, PackRegistryIndex>(&query)
            .bind(id)
            .bind(update_name)
            .bind(name)
            .bind(url)
            .bind(input.position)
            .bind(input.enabled)
            .bind(input.headers)
            .fetch_one(executor)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => {
                    Error::not_found("pack_registry_index", "id", id.to_string())
                }
                other => other.into(),
            })
    }
}

fn normalize_url(url: &str) -> Result<String> {
    let mut parsed =
        url::Url::parse(url).map_err(|e| Error::validation(format!("Invalid index URL: {}", e)))?;
    if parsed.scheme() != "https" {
        return Err(Error::validation("API-managed index URLs must use HTTPS"));
    }
    if parsed.host_str().is_none() {
        return Err(Error::validation("Index URL must include a host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(Error::validation(
            "Index URLs must not contain credentials or fragments",
        ));
    }
    if parsed.query().is_some() {
        return Err(Error::validation(
            "Managed index URLs must not contain query parameters; use encrypted headers for credentials",
        ));
    }
    let host = parsed
        .host_str()
        .unwrap()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    parsed
        .set_host(Some(&host))
        .map_err(|_| Error::validation("Index URL has an invalid host"))?;
    if parsed.port().is_some_and(|port| port == 443) {
        parsed
            .set_port(None)
            .map_err(|_| Error::validation("Index URL has an invalid port"))?;
    }
    Ok(parsed.to_string())
}

fn validate_encrypted_headers(headers: &serde_json::Value) -> Result<()> {
    let is_empty = headers.as_object().is_some_and(serde_json::Map::is_empty);
    if (!headers.is_string() && !is_empty) || headers.as_str() == Some("[REDACTED]") {
        return Err(Error::validation(
            "Managed registry headers must be empty or encrypted before persistence",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_indices_require_clean_https_urls() {
        assert!(normalize_url("https://registry.example/index.json").is_ok());
        assert!(normalize_url("file:///tmp/index.json").is_err());
        assert!(normalize_url("http://registry.example/index.json").is_err());
        assert!(normalize_url("https://user:secret@registry.example/index.json").is_err());
        assert!(normalize_url("https://registry.example/index.json#fragment").is_err());
        assert!(normalize_url("https://registry.example/index.json?token=secret").is_err());
        assert_eq!(
            normalize_url("HTTPS://REGISTRY.EXAMPLE.:443/index.json").unwrap(),
            "https://registry.example/index.json"
        );
    }

    #[test]
    fn managed_headers_must_be_encrypted_before_repository_write() {
        assert!(validate_encrypted_headers(&serde_json::json!({})).is_ok());
        assert!(validate_encrypted_headers(&serde_json::json!({
            "Authorization": "Bearer secret"
        }))
        .is_err());
        assert!(validate_encrypted_headers(&serde_json::json!("ciphertext")).is_ok());
        assert!(validate_encrypted_headers(&serde_json::json!("[REDACTED]")).is_err());
    }
}
