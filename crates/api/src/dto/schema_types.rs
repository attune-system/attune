use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct SchemaTypeResponse {
    pub id: i64,
    pub r#ref: String,
    pub pack_ref: String,
    pub label: String,
    pub description: Option<String>,
    pub schema: Value,
    /// Pack-reference policy, not a restriction on authenticated inspection or
    /// Key schema assignment.
    pub reference_visibility: String,
    pub reference_allowed_pack_refs: Vec<String>,
    pub managed_release: Option<i64>,
}

impl From<attune_common::models::schema_type::SchemaType> for SchemaTypeResponse {
    fn from(value: attune_common::models::schema_type::SchemaType) -> Self {
        Self {
            id: value.id,
            r#ref: value.r#ref,
            pack_ref: value.pack_ref,
            label: value.label,
            description: value.description,
            schema: value.schema,
            reference_visibility: format!("{:?}", value.reference_visibility).to_ascii_lowercase(),
            reference_allowed_pack_refs: value.reference_allowed_pack_refs,
            managed_release: value.managed_release,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchemaPreviewContext {
    FlatDocument,
    SingleValue,
}

impl From<SchemaPreviewContext> for attune_common::value_schema::SchemaContext {
    fn from(context: SchemaPreviewContext) -> Self {
        match context {
            SchemaPreviewContext::FlatDocument => Self::FlatDocument,
            SchemaPreviewContext::SingleValue => Self::SingleValue,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaPreviewRequest {
    pub context: SchemaPreviewContext,
    pub schema: Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SchemaContractResponse {
    /// Self-contained rendering graph. Preserve authored references in editors.
    pub contract: Value,
    pub semantic_digest: String,
}

impl SchemaContractResponse {
    pub fn from_contract(
        contract: attune_common::value_schema::Contract,
    ) -> attune_common::Result<Self> {
        let semantic_digest = contract.semantic_digest().map_err(|error| {
            attune_common::Error::validation(format!("Invalid schema preview digest: {error}"))
        })?;
        let contract = serde_json::to_value(contract)
            .map_err(|_| attune_common::Error::validation("Invalid schema preview contract"))?;
        Ok(Self {
            contract,
            semantic_digest,
        })
    }
}
