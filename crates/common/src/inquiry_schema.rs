//! Inquiry option and response materialization under one fixed response contract.
//! Persistence adapters select and retain this binding at inquiry creation.
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;

use crate::{
    models::inquiry::InquiryResponseOption,
    secret_provenance::SecretProvenance,
    secret_values::{merge_contract_secret_redactions, SecretValueInput},
    value_schema::{CompiledContract, Contract, SchemaContext, SourceIdentity},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InquirySchemaBinding {
    pub response_schema: Option<Value>,
    pub contract: Contract,
}

/// Idempotent creation compares canonical requests without exposing a plain
/// hash of potentially low-entropy secret option values. This is not a public
/// schema fingerprint or a revision-selection input.
pub fn fingerprint_request(request: &impl Serialize, encryption_key: &str) -> Result<String> {
    let bytes = serde_jcs::to_vec(request)
        .map_err(|_| Error::validation("Invalid inquiry admission request"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(encryption_key.as_bytes())
        .map_err(|_| Error::validation("Invalid inquiry admission key"))?;
    mac.update(b"attune:inquiry-admission:v1\0");
    mac.update(&bytes);
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

impl InquirySchemaBinding {
    pub fn inline(response_schema: Option<Value>, component_ref: String) -> Result<Self> {
        let unrestricted = Value::Bool(true);
        let context = if response_schema.is_some() {
            SchemaContext::FlatDocument
        } else {
            SchemaContext::SingleValue
        };
        let contract = crate::value_schema::compile(
            SourceIdentity::Assigned {
                component_ref,
                assignment_id: uuid::Uuid::new_v4(),
            },
            context,
            response_schema.as_ref().unwrap_or(&unrestricted),
            &crate::value_schema::RegistryView::new(),
            crate::value_schema::Limits::default(),
        )
        .map_err(|error| Error::validation(format!("Invalid inline inquiry schema: {error}")))?;
        Ok(Self {
            response_schema,
            contract,
        })
    }
    pub fn validate(&self) -> Result<()> {
        self.compiled().map(|_| ())
    }

    fn compiled(&self) -> Result<CompiledContract<'_>> {
        let unrestricted = Value::Bool(true);
        let (context, schema) = match &self.response_schema {
            Some(schema) => (SchemaContext::FlatDocument, schema),
            None => (SchemaContext::SingleValue, &unrestricted),
        };
        if matches!(self.contract.source, SourceIdentity::Authoring { .. })
            || self.contract.context != context
            || crate::value_schema::digest(schema)
                .map_err(|_| Error::validation("Invalid inquiry schema source"))?
                != self.contract.authored_digest
        {
            return Err(Error::validation(
                "Inquiry requires its original authoritative response contract",
            ));
        }
        CompiledContract::new(&self.contract).map_err(|error| {
            Error::validation(format!("Invalid inquiry response contract: {error}"))
        })
    }

    pub fn prepare_response(
        &self,
        response: &Value,
        inherited: &SecretProvenance,
    ) -> Result<(Value, Vec<SecretValueInput>)> {
        let contract = self.compiled()?;
        Self::prepare_response_compiled(&contract, response, inherited)
    }

    fn prepare_response_compiled(
        contract: &CompiledContract<'_>,
        response: &Value,
        inherited: &SecretProvenance,
    ) -> Result<(Value, Vec<SecretValueInput>)> {
        let response = contract
            .apply_defaults(response)
            .map_err(|error| Error::validation(format!("Invalid inquiry response: {error}")))?;
        contract
            .validate(&response)
            .map_err(|error| Error::validation(format!("Invalid inquiry response: {error}")))?;
        let (response, mut inputs) = merge_contract_secret_redactions(response, &[], contract)?;
        if !inherited.origins.is_empty() {
            let origins = serde_json::to_string(inherited)
                .map_err(|_| Error::validation("Invalid inquiry response provenance"))?;
            for input in &mut inputs {
                input.source_kind = "provenance".into();
                input.source_ref = Some(origins.clone());
            }
        }
        Ok((response, inputs))
    }

    pub fn prepare_options(
        &self,
        options: &[InquiryResponseOption],
        inherited: &SecretProvenance,
    ) -> Result<(Vec<InquiryResponseOption>, Vec<SecretValueInput>)> {
        let contract = self.compiled()?;
        // Structural checks precede default insertion. Acceptance checks every
        // effective response below rather than relaxing schema assertions.
        crate::inquiry_options::validate_bound_response_options(None, options)?;
        let mut prepared = Vec::with_capacity(options.len());
        let mut secrets = Vec::new();
        let mut effective_bytes = 2usize;
        for (index, option) in options.iter().enumerate() {
            let response = contract.apply_defaults(&option.response).map_err(|error| {
                Error::validation(format!("Invalid inquiry option defaults: {error}"))
            })?;
            let mut effective = option.clone();
            effective.response = response;
            effective_bytes = effective_bytes.saturating_add(
                serde_json::to_vec(&effective)
                    .map_err(|_| Error::validation("Invalid inquiry response option"))?
                    .len()
                    .saturating_add(1),
            );
            if effective_bytes > crate::inquiry_options::MAX_RESPONSE_OPTIONS_SERIALIZED_SIZE {
                return Err(Error::validation(
                    "Effective inquiry options exceed their serialized budget",
                ));
            }
            let (response, mut inputs) =
                Self::prepare_response_compiled(&contract, &effective.response, inherited)?;
            for input in &mut inputs {
                input.json_path = format!("/{index}/response{}", input.json_path);
            }
            let mut option = option.clone();
            option.response = response;
            prepared.push(option);
            secrets.extend(inputs);
        }
        Ok((prepared, secrets))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::inquiry::InquiryResponseOptionStyle,
        value_schema::{compile, Limits, RegistryView},
    };
    use serde_json::json;

    fn binding(schema: Option<Value>) -> InquirySchemaBinding {
        let unrestricted = json!(true);
        let context = if schema.is_some() {
            SchemaContext::FlatDocument
        } else {
            SchemaContext::SingleValue
        };
        let contract = compile(
            SourceIdentity::Assigned {
                component_ref: "test.inquiry".into(),
                assignment_id: uuid::Uuid::new_v4(),
            },
            context,
            schema.as_ref().unwrap_or(&unrestricted),
            &RegistryView::new(),
            Limits::default(),
        )
        .unwrap();
        InquirySchemaBinding {
            response_schema: schema,
            contract,
        }
    }

    #[test]
    fn options_get_public_defaults_and_secret_values_are_separate_from_handles_and_labels() {
        let binding = binding(Some(
            json!({"token": {"type": "string", "secret": true}, "approved": {"type": "boolean", "default": false, "required": true}}),
        ));
        let options = vec![InquiryResponseOption {
            r#ref: "approve".into(),
            label: "Approve".into(),
            style: InquiryResponseOptionStyle::Default,
            response: json!({"token": "private-option"}),
        }];
        let (prepared, secrets) = binding
            .prepare_options(&options, &SecretProvenance::default())
            .unwrap();
        assert_eq!(prepared[0].response["approved"], false);
        assert!(crate::secret_values::is_redaction_marker(
            &prepared[0].response["token"]
        ));
        assert_eq!(prepared[0].label, "Approve");
        assert_eq!(secrets[0].json_path, "/0/response/token");
        assert_eq!(secrets[0].value, "private-option");
        assert!(binding
            .prepare_response(&json!({"approved": null}), &SecretProvenance::default())
            .is_err());
    }

    #[test]
    fn admission_fingerprints_are_keyed_and_effective_option_budgets_include_defaults() {
        let request = json!({"secret": "low-entropy", "purpose": "approve"});
        let reordered = json!({"purpose": "approve", "secret": "low-entropy"});
        assert_eq!(
            fingerprint_request(&request, "key-one").unwrap(),
            fingerprint_request(&reordered, "key-one").unwrap()
        );
        assert_ne!(
            fingerprint_request(&request, "key-one").unwrap(),
            fingerprint_request(&request, "key-two").unwrap()
        );
        let binding = binding(Some(
            json!({"description": {"type": "string", "default": "x".repeat(70_000)}}),
        ));
        let option = InquiryResponseOption {
            r#ref: "approve".into(),
            label: "Approve".into(),
            style: InquiryResponseOptionStyle::Default,
            response: json!({}),
        };
        assert!(binding
            .prepare_options(&[option], &SecretProvenance::default())
            .is_err());
    }

    #[test]
    fn schema_less_responses_are_unrestricted_and_preview_sources_are_rejected() {
        let mut binding = binding(None);
        assert_eq!(
            binding
                .prepare_response(&json!("free-form"), &SecretProvenance::default())
                .unwrap()
                .0,
            "free-form"
        );
        binding.contract.source = SourceIdentity::Authoring {
            component_ref: "preview".into(),
        };
        assert!(binding.validate().is_err());
    }
}
