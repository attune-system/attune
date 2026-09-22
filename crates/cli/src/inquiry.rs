use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::{ApiClient, PaginatedResponse};

#[derive(Debug, Clone, Default)]
pub struct InquiryListFilters {
    pub status: Option<String>,
    pub created_by_execution: Option<i64>,
    pub assigned_to: Option<i64>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InquirySummary {
    pub id: i64,
    pub created_by_execution: i64,
    pub prompt: String,
    pub assigned_to: Option<i64>,
    pub status: String,
    pub has_response: bool,
    pub timeout_at: Option<String>,
    pub created: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InquiryResponseOption {
    #[serde(rename = "ref")]
    pub option_ref: String,
    pub label: String,
    pub style: String,
    pub response: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inquiry {
    pub id: i64,
    pub created_by_execution: i64,
    pub workflow_execution: Option<i64>,
    pub workflow_task_name: Option<String>,
    pub purpose: Option<String>,
    pub prompt: String,
    pub response_schema: Option<Value>,
    #[serde(default)]
    pub response_options: Vec<InquiryResponseOption>,
    pub assigned_to: Option<i64>,
    pub status: String,
    pub response: Option<Value>,
    pub timeout_at: Option<String>,
    pub responded_by: Option<i64>,
    pub responded_at: Option<String>,
    pub created: String,
    pub updated: String,
}

pub async fn list(
    client: &mut ApiClient,
    filters: &InquiryListFilters,
) -> Result<PaginatedResponse<InquirySummary>> {
    let mut query = vec![
        format!("offset={}", filters.offset),
        format!("limit={}", filters.limit),
    ];
    if let Some(status) = &filters.status {
        query.push(format!("status={}", urlencoding::encode(status)));
    }
    if let Some(execution_id) = filters.created_by_execution {
        query.push(format!("created_by_execution={execution_id}"));
    }
    if let Some(identity_id) = filters.assigned_to {
        query.push(format!("assigned_to={identity_id}"));
    }

    client
        .get_paginated_response(&format!("/inquiries?{}", query.join("&")))
        .await
}

pub async fn get(client: &mut ApiClient, id: i64) -> Result<Inquiry> {
    client.get(&format!("/inquiries/{id}")).await
}

pub async fn respond(client: &mut ApiClient, id: i64, response: Value) -> Result<Inquiry> {
    if !response.is_object() {
        bail!("Inquiry response must be a JSON object");
    }
    client
        .post(
            &format!("/inquiries/{id}/respond"),
            &json!({ "response": response }),
        )
        .await
}

pub async fn respond_with_option(
    client: &mut ApiClient,
    id: i64,
    option_ref: &str,
) -> Result<Inquiry> {
    let inquiry = get(client, id).await?;
    let response = inquiry
        .response_options
        .iter()
        .find(|option| option.option_ref == option_ref)
        .map(|option| option.response.clone())
        .ok_or_else(|| anyhow!("Inquiry {id} has no response option with ref '{option_ref}'"))?;
    respond(client, id, response).await
}

pub async fn create(client: &mut ApiClient, request: Value) -> Result<Value> {
    if !request.is_object() {
        bail!("Inquiry create request must be a JSON object");
    }
    client.post("/inquiries", &request).await
}

pub async fn cancel(client: &mut ApiClient, id: i64) -> Result<Inquiry> {
    client
        .post(&format!("/inquiries/{id}/cancel"), &json!({}))
        .await
}
