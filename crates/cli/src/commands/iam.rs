use anyhow::{bail, Result};
use clap::Args;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{ApiClient, ApiRequestError, PaginatedResponse};
use crate::output::{self, OutputFormat};

#[derive(Debug, Args)]
#[group(id = "identity_selector", required = true, multiple = false)]
pub struct IdentitySelector {
    /// Exact identity login. Use --identity-id for a database ID.
    pub login: Option<String>,
    /// Identity database ID
    #[arg(long)]
    pub identity_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentitySummary {
    pub id: i64,
    pub login: String,
    pub display_name: Option<String>,
    pub frozen: bool,
    pub attributes: Value,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: i64,
    pub login: String,
    pub display_name: Option<String>,
    pub frozen: bool,
    pub attributes: Value,
    pub roles: Vec<IdentityRole>,
    pub direct_permissions: Vec<DirectPermission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRole {
    pub id: i64,
    pub identity_id: i64,
    pub role: String,
    pub source: String,
    pub managed: bool,
    pub created: String,
    pub updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectPermission {
    pub id: i64,
    pub identity_id: i64,
    pub permission_set_id: i64,
    pub permission_set_ref: String,
    pub created: String,
}

#[derive(Debug, Serialize)]
pub struct MutationResult {
    pub operation: &'static str,
    pub target: Value,
    pub dry_run: bool,
    pub changed: bool,
    pub would_change: bool,
    pub before: Value,
    pub after: Value,
}

impl MutationResult {
    pub fn new(
        operation: &'static str,
        target: Value,
        dry_run: bool,
        before: Value,
        after: Value,
    ) -> Self {
        let would_change = before != after;
        Self {
            operation,
            target,
            dry_run,
            changed: !dry_run && would_change,
            would_change,
            before,
            after,
        }
    }

    pub fn print(&self, format: OutputFormat) -> Result<()> {
        output::print_output(self, format)
    }
}

pub fn query_path(path: &str, pairs: &[(&str, String)]) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().map(|(key, value)| (*key, value.as_str())))
        .finish();
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    }
}

pub async fn resolve_identity(
    client: &mut ApiClient,
    login: Option<&str>,
    identity_id: Option<i64>,
) -> Result<Identity> {
    let id = match (login, identity_id) {
        (None, Some(id)) => id,
        (Some(login), None) => {
            let path = query_path("/identities", &[("login", login.to_string())]);
            let mut identities: Vec<IdentitySummary> = client.get_paginated(&path).await?;
            identities
                .pop()
                .ok_or_else(|| anyhow::anyhow!("Identity '{login}' not found"))?
                .id
        }
        _ => bail!("Specify exactly one identity login or --identity-id"),
    };
    client.get(&format!("/identities/{id}")).await
}

pub fn has_status(error: &anyhow::Error, status: StatusCode) -> bool {
    error
        .downcast_ref::<ApiRequestError>()
        .is_some_and(|error| error.status == status)
}

pub fn validate_role(role: &str) -> Result<()> {
    if role.trim().is_empty() || role.chars().count() > 255 {
        bail!("Role name must contain 1 to 255 characters and cannot be blank");
    }
    Ok(())
}

pub async fn delete_if_present(client: &mut ApiClient, path: &str) -> Result<bool> {
    match client.delete_no_response(path).await {
        Ok(()) => Ok(true),
        Err(error) if has_status(&error, StatusCode::NOT_FOUND) => Ok(false),
        Err(error) => Err(error),
    }
}

pub async fn all_pages<T: serde::de::DeserializeOwned>(
    client: &mut ApiClient,
    path: &str,
    filters: &[(&str, String)],
) -> Result<Vec<T>> {
    let mut items = Vec::new();
    let mut page = 1;
    loop {
        let mut query = filters.to_vec();
        query.extend([("page", page.to_string()), ("page_size", "100".to_string())]);
        let response: PaginatedResponse<T> = client
            .get_paginated_response(&query_path(path, &query))
            .await?;
        let has_next = response.pagination["has_next"]
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("API pagination is missing has_next"))?;
        items.extend(response.items);
        if !has_next {
            return Ok(items);
        }
        page += 1;
    }
}
