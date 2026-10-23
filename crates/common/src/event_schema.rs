//! Immutable admission contracts for the two event value sections.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    secret_values::{redact_secret_path_sources, SecretPathSource, SecretSource, SecretValueInput},
    value_schema::{CompiledContract, Contract, SchemaContext, SourceIdentity},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSchemaBinding {
    pub trigger_id: Option<i64>,
    pub trigger_ref: String,
    pub payload: Option<Contract>,
    pub config: Option<Contract>,
    pub webhook: bool,
}

pub struct PreparedEventSections {
    pub payload: Option<Value>,
    pub config: Option<Value>,
    pub payload_secrets: Vec<SecretValueInput>,
    pub config_secrets: Vec<SecretValueInput>,
}

impl PreparedEventSections {
    /// Runtime plaintext can be copied into an undeclared public field. Protect
    /// whole normalized sections, not only the trigger's declared secret paths.
    pub fn inherit_runtime_reads(
        &mut self,
        origins: &[crate::secret_provenance::SecretOrigin],
    ) -> Result<()> {
        if origins.is_empty() {
            return Ok(());
        }
        for (section, secrets) in [
            (&mut self.payload, &mut self.payload_secrets),
            (&mut self.config, &mut self.config_secrets),
        ] {
            let Some(mut value) = section.take() else {
                continue;
            };
            let mut provenance = crate::secret_provenance::SecretProvenance {
                origins: origins.to_vec(),
                templates: vec![],
            };
            for secret in std::mem::take(secrets) {
                if secret.json_path.is_empty() {
                    value = secret.value;
                } else {
                    *value
                        .pointer_mut(&secret.json_path)
                        .ok_or_else(|| Error::validation("Event secret path is unavailable"))? =
                        secret.value;
                }
                if secret.source_kind != "provenance" {
                    return Err(Error::validation("Event secret provenance is unavailable"));
                }
                provenance.merge(serde_json::from_str(
                    secret.source_ref.as_deref().ok_or_else(|| {
                        Error::validation("Event secret provenance is unavailable")
                    })?,
                )?);
            }
            let reference = serde_json::to_string(&provenance)?;
            if provenance.origins.len() > 256 || reference.len() > 1024 * 1024 {
                return Err(Error::validation("Event read provenance exceeds its bound"));
            }
            secrets.push(SecretValueInput {
                json_path: String::new(),
                value,
                source_kind: "provenance".into(),
                source_ref: Some(reference),
            });
            *section = Some(crate::secret_values::redaction_marker());
        }
        Ok(())
    }
}

#[cfg(test)]
mod runtime_read_event_tests {
    use super::*;
    #[test]
    fn public_event_fields_cannot_drop_runtime_origins() {
        let origin = crate::repositories::runtime_read::RuntimeReadRepository::reference(7);
        let mut sections = PreparedEventSections {
            payload: Some(serde_json::json!({"public":"sdk-secret"})),
            config: Some(serde_json::json!({"public":"sdk-secret"})),
            payload_secrets: vec![],
            config_secrets: vec![],
        };
        sections
            .inherit_runtime_reads(std::slice::from_ref(&origin))
            .unwrap();
        for (value, inputs) in [
            (sections.payload, sections.payload_secrets),
            (sections.config, sections.config_secrets),
        ] {
            assert!(!value.unwrap().to_string().contains("sdk-secret"));
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].json_path, "");
            let provenance: crate::secret_provenance::SecretProvenance =
                serde_json::from_str(inputs[0].source_ref.as_deref().unwrap()).unwrap();
            assert_eq!(provenance.origins, vec![origin.clone()]);
        }
    }
}

impl EventSchemaBinding {
    /// Trigger removal clears the nullable projection FK, not the recorded
    /// admission identity. Never resolve a replacement component by reference.
    pub fn matches_owner(&self, event: &crate::models::event::Event) -> bool {
        self.matches_identity(event.trigger, &event.trigger_ref)
    }

    pub fn matches_identity(&self, trigger: Option<i64>, trigger_ref: &str) -> bool {
        self.trigger_ref == trigger_ref && trigger.is_none_or(|id| self.trigger_id == Some(id))
    }

    pub fn validate(&self) -> Result<()> {
        if self.webhook
            && self
                .config
                .as_ref()
                .is_none_or(|contract| !contract.dependencies.is_empty())
        {
            return Err(Error::validation(
                "Webhook metadata requires the server-owned contract",
            ));
        }
        if self
            .payload
            .as_ref()
            .is_some_and(|contract| contract.source.component_ref() != self.trigger_ref)
            || self.config.as_ref().is_some_and(|contract| {
                contract.source.component_ref()
                    != if self.webhook {
                        "system.webhook-event-config"
                    } else {
                        &self.trigger_ref
                    }
            })
        {
            return Err(Error::validation(
                "Event section source does not match its trigger",
            ));
        }
        for contract in [&self.payload, &self.config].into_iter().flatten() {
            if contract.context != SchemaContext::FlatDocument
                || matches!(contract.source, SourceIdentity::Authoring { .. })
            {
                return Err(Error::validation("Invalid event section contract"));
            }
            CompiledContract::new(contract)
                .map_err(|_| Error::validation("Invalid event section binding"))?;
        }
        Ok(())
    }

    /// Validate plaintext and apply public defaults before deriving encryption.
    /// An absent optional config section stays absent. A declared payload
    /// contract still enforces required fields when the payload is omitted.
    pub fn prepare(
        &self,
        payload: Option<Value>,
        config: Option<Value>,
    ) -> Result<PreparedEventSections> {
        self.validate()?;
        if self.webhook && config.is_none() {
            return Err(Error::validation("Webhook metadata is required"));
        }
        let payload = if self.payload.is_some() && payload.is_none() {
            Some(json!({}))
        } else {
            payload
        };
        let (payload, payload_secrets) =
            prepare_section(&self.trigger_ref, "payload", self.payload.as_ref(), payload)?;
        let (config, config_secrets) =
            prepare_section(&self.trigger_ref, "config", self.config.as_ref(), config)?;
        Ok(PreparedEventSections {
            payload,
            config,
            payload_secrets,
            config_secrets,
        })
    }

    pub fn validate_restored(&self, payload: Option<&Value>, config: Option<&Value>) -> Result<()> {
        self.validate()?;
        for (contract, value, required_section) in [
            (&self.payload, payload, true),
            (&self.config, config, self.webhook),
        ] {
            validate_restored_section(contract.as_ref(), value, required_section)?;
        }
        Ok(())
    }

    /// Payload-only consumers do not restore transport metadata. Validate the
    /// section they actually read without treating unread metadata as missing.
    pub fn validate_payload_restored(&self, payload: Option<&Value>) -> Result<()> {
        self.validate()?;
        validate_restored_section(self.payload.as_ref(), payload, true)
    }
}

fn validate_restored_section(
    contract: Option<&Contract>,
    value: Option<&Value>,
    required: bool,
) -> Result<()> {
    let Some(contract) = contract else {
        return Ok(());
    };
    let compiled = CompiledContract::new(contract)
        .map_err(|_| Error::validation("Invalid event section binding"))?;
    let empty = json!({});
    if let Some(value) = value.or(required.then_some(&empty)) {
        compiled
            .validate(value)
            .map_err(|error| Error::validation(format!("Invalid bound event value: {error}")))?;
    }
    Ok(())
}

fn prepare_section(
    trigger_ref: &str,
    section: &'static str,
    contract: Option<&Contract>,
    value: Option<Value>,
) -> Result<(Option<Value>, Vec<SecretValueInput>)> {
    let Some(value) = value else {
        return Ok((None, Vec::new()));
    };
    let Some(contract) = contract else {
        return Ok((Some(value), Vec::new()));
    };
    let compiled = CompiledContract::new(contract)
        .map_err(|_| Error::validation("Invalid event section binding"))?;
    let value = compiled
        .apply_defaults(&value)
        .map_err(|error| Error::validation(format!("Invalid event defaults: {error}")))?;
    compiled
        .validate(&value)
        .map_err(|error| Error::validation(format!("Invalid event value: {error}")))?;
    let sources = compiled
        .secret_paths(&value)
        .map_err(|error| Error::validation(format!("Invalid event secret paths: {error}")))?
        .into_iter()
        .map(|path| SecretPathSource {
            source: SecretSource::TriggerSchema {
                trigger_ref: Some(trigger_ref.to_owned()),
                section,
                path: path.clone(),
            },
            path,
        })
        .collect::<Vec<_>>();
    let (value, secrets) = redact_secret_path_sources(value, &sources);
    Ok((Some(value), secrets))
}

/// Server-owned transport metadata does not use the trigger's instance schema
/// or its sensor-defined event configuration schema. Headers are protected as
/// a whole because arbitrary header names can carry credentials.
pub fn webhook_config_schema() -> Value {
    json!({
        "source": {"type": "string", "const": "webhook", "required": true},
        "webhook_key": {"type": "string", "secret": true, "required": true},
        "received_at": {"type": "string", "format": "date-time", "required": true},
        "headers": {"type": "object", "secret": true},
        "source_ip": {"type": "string"},
        "user_agent": {"type": "string"},
        "hmac_verified": {"type": "boolean"}
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value_schema::{compile, Limits, RegistryView};

    fn contract(reference: &str, schema: &Value) -> Contract {
        compile(
            SourceIdentity::Assigned {
                component_ref: reference.into(),
                assignment_id: uuid::Uuid::new_v4(),
            },
            SchemaContext::FlatDocument,
            schema,
            &RegistryView::new(),
            Limits::default(),
        )
        .unwrap()
    }

    #[test]
    fn webhook_payload_consumers_do_not_require_unread_transport_credentials() {
        let binding = EventSchemaBinding {
            trigger_id: Some(1),
            trigger_ref: "test.webhook".into(),
            webhook: true,
            payload: Some(contract(
                "test.webhook",
                &json!({"message":{"type":"string","required":true}}),
            )),
            config: Some(contract(
                "system.webhook-event-config",
                &webhook_config_schema(),
            )),
        };
        let payload = json!({"message":"webhook fixture"});
        let metadata = json!({"source":"webhook","webhook_key":"transport-secret-fixture","received_at":"2026-10-14T02:00:00Z"});
        binding
            .prepare(Some(payload.clone()), Some(metadata.clone()))
            .unwrap();
        binding.validate_payload_restored(Some(&payload)).unwrap();
        assert!(binding
            .validate_payload_restored(Some(&json!({"message":false})))
            .is_err());
        assert!(binding.validate_payload_restored(None).is_err());
        assert!(binding.validate_restored(Some(&payload), None).is_err());
        binding
            .validate_restored(Some(&payload), Some(&metadata))
            .unwrap();
    }

    #[test]
    fn sections_apply_defaults_validate_strictly_and_preserve_absent_config() {
        let binding = EventSchemaBinding {
            trigger_id: Some(1),
            trigger_ref: "test.event".into(),
            webhook: false,
            payload: Some(contract(
                "test.event",
                &json!({"count": {"type": "integer", "required": true, "default": 0}, "credentials": {"type": "array", "items": {"type": "object", "properties": {"token": {"type": "string", "secret": true}}}}}),
            )),
            config: Some(contract(
                "test.event",
                &json!({"enabled": {"type": "boolean", "required": true}}),
            )),
        };
        let prepared = binding
            .prepare(
                Some(json!({"count": 0, "credentials": [{"token": "private"}]})),
                None,
            )
            .unwrap();
        assert_eq!(prepared.payload.as_ref().unwrap()["count"], 0);
        assert!(prepared.config.is_none());
        assert_eq!(
            prepared.payload_secrets[0].json_path,
            "/credentials/0/token"
        );
        assert_eq!(
            binding.prepare(None, None).unwrap().payload.unwrap()["count"],
            0
        );
        assert!(binding
            .prepare(Some(json!({"count": "{{ value }}"})), None)
            .is_err());
        assert!(binding.prepare(Some(json!({})), Some(json!({}))).is_err());
        binding
            .prepare(Some(json!({})), Some(json!({"enabled": false})))
            .unwrap();
    }

    #[test]
    fn webhook_metadata_credentials_and_headers_are_protected_independently_of_payload() {
        let binding = EventSchemaBinding {
            trigger_id: Some(1),
            trigger_ref: "test.webhook".into(),
            webhook: true,
            payload: None,
            config: Some(contract(
                "system.webhook-event-config",
                &webhook_config_schema(),
            )),
        };
        let config = json!({"source": "webhook", "webhook_key": "private-key", "received_at": "today", "headers": {"Authorization": "Bearer private", "Cookie": "private-cookie"}, "hmac_verified": false});
        let prepared = binding
            .prepare(Some(json!("unrestricted payload")), Some(config.clone()))
            .unwrap();
        assert_eq!(prepared.payload, Some(json!("unrestricted payload")));
        assert_eq!(prepared.config_secrets.len(), 2);
        let public = serde_json::to_string(&prepared.config).unwrap();
        assert!(!public.contains("private-key"));
        assert!(!public.contains("Authorization"));
        assert_eq!(prepared.config.as_ref().unwrap()["hmac_verified"], false);
        binding
            .validate_restored(prepared.payload.as_ref(), Some(&config))
            .unwrap();
        let mut forged = binding;
        forged.config = None;
        assert!(forged.validate().is_err());
    }

    #[test]
    fn historical_webhook_contracts_do_not_compare_against_todays_builtin_definition() {
        let mut schema = webhook_config_schema();
        schema["user_agent"]["description"] = json!("An earlier transport contract");
        let binding = EventSchemaBinding {
            trigger_id: Some(1),
            trigger_ref: "test.webhook".into(),
            webhook: true,
            payload: None,
            config: Some(contract("system.webhook-event-config", &schema)),
        };
        binding
            .prepare(
                None,
                Some(
                    json!({"source": "webhook", "webhook_key": "private", "received_at": "today"}),
                ),
            )
            .unwrap();
    }
}
