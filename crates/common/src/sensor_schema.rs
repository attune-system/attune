//! Self-contained, system-owned contracts for one admitted sensor generation.

use crate::{
    event_schema::EventSchemaBinding,
    models::SensorExecutableSnapshot,
    value_schema::{CompiledContract, Contract, SchemaContext, SourceIdentity},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorTriggerBinding {
    pub parameters: Option<Contract>,
    pub event: EventSchemaBinding,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorGenerationBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_revision: Option<i64>,
    pub executable: SensorExecutableSnapshot,
    pub config: Contract,
    pub triggers: BTreeMap<String, SensorTriggerBinding>,
}

impl SensorGenerationBinding {
    /// A pack-scoped sensor can fetch its pack's Keys through the SDK even when
    /// no input schema marks secrets. Its diagnostics must remain protected.
    pub fn requires_protected_logs(&self) -> Result<bool> {
        Ok(self.executable.sensor.pack_ref.is_some() || self.declares_secrets()?)
    }
    pub fn binding_digest(&self) -> Result<String> {
        crate::value_schema::digest(self)
            .map_err(|_| Error::validation("Invalid sensor generation digest"))
    }
    pub fn declares_secrets(&self) -> Result<bool> {
        for contract in
            std::iter::once(&self.config).chain(self.triggers.values().flat_map(|binding| {
                [
                    &binding.parameters,
                    &binding.event.payload,
                    &binding.event.config,
                ]
                .into_iter()
                .flatten()
            }))
        {
            if CompiledContract::new(contract)
                .map_err(|_| Error::validation("Invalid sensor generation contract"))?
                .declares_secrets()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn validate(&self) -> Result<()> {
        if self
            .executable
            .input_permission_set_refs
            .iter()
            .any(|reference| reference.trim() == "standard")
            || self.executable.input_grants.iter().any(|grant| {
                !matches!(
                    grant.resource,
                    crate::rbac::Resource::Keys | crate::rbac::Resource::Packs
                ) || grant.actions.is_empty()
                    || grant.actions.iter().any(|action| {
                        !matches!(
                            action,
                            crate::rbac::Action::Read | crate::rbac::Action::Decrypt
                        )
                    })
            })
        {
            return Err(Error::validation("Invalid sensor input authority"));
        }
        if self
            .executable
            .signing_permission_set_refs
            .iter()
            .any(|reference| reference.trim() == "standard")
            || self.executable.signing_grants.iter().any(|grant| {
                grant.resource != crate::rbac::Resource::Keys
                    || grant.actions != [crate::rbac::Action::Use]
            })
        {
            return Err(Error::validation("Invalid sensor signing authority"));
        }
        if self
            .executable
            .cache_grants
            .iter()
            .any(|grant| grant.resource != crate::rbac::Resource::Caches)
        {
            return Err(Error::validation("Invalid sensor cache authority"));
        }
        let expected = SourceIdentity::Released {
            component_ref: self.executable.sensor.r#ref.clone(),
            release_id: self.executable.release.id,
            release_digest: self.executable.release.digest.clone(),
        };
        let config_source = self.config.source == expected
            || matches!(&self.config.source,
            SourceIdentity::Assigned {component_ref,..} if self.metadata_revision.is_some_and(|id|id>0) && component_ref==&self.executable.sensor.r#ref);
        if !config_source || self.config.context != SchemaContext::FlatDocument {
            return Err(Error::validation(
                "Sensor config contract does not match its executable",
            ));
        }
        CompiledContract::new(&self.config)
            .map_err(|_| Error::validation("Invalid sensor config contract"))?;
        if crate::value_schema::digest(
            self.executable
                .sensor
                .param_schema
                .as_ref()
                .unwrap_or(&serde_json::json!({})),
        )
        .map_err(|_| Error::validation("Invalid sensor authored schema"))?
            != self.config.authored_digest
        {
            return Err(Error::validation(
                "Sensor configuration schema digest mismatch",
            ));
        }
        if self.executable.sensor.config.is_some()
            || self.triggers.len() != self.executable.triggers.len()
        {
            return Err(Error::validation("Invalid sensor generation metadata"));
        }
        for (reference, binding) in &self.triggers {
            let trigger = self
                .executable
                .triggers
                .iter()
                .find(|trigger| &trigger.r#ref == reference)
                .ok_or_else(|| Error::validation("Sensor trigger snapshot is unavailable"))?;
            if reference != &binding.event.trigger_ref || binding.event.webhook {
                return Err(Error::validation("Invalid admitted sensor trigger"));
            }
            if binding.event.trigger_id != Some(trigger.id)
                || trigger.webhook_key.is_some()
                || trigger.webhook_config.is_some()
            {
                return Err(Error::validation(
                    "Invalid sensor trigger identity or metadata",
                ));
            }
            for contract in [
                &binding.parameters,
                &binding.event.payload,
                &binding.event.config,
            ]
            .into_iter()
            .flatten()
            {
                let source_matches = match &contract.source {
                    SourceIdentity::Released {
                        component_ref,
                        release_id,
                        release_digest,
                    } => {
                        component_ref == reference
                            && if trigger.pack_ref == self.executable.sensor.pack_ref {
                                *release_id == self.executable.release.id
                                    && release_digest == &self.executable.release.digest
                            } else {
                                false
                            }
                    }
                    SourceIdentity::Assigned { component_ref, .. } => {
                        component_ref == reference
                            && trigger.pack_ref != self.executable.sensor.pack_ref
                    }
                    _ => false,
                };
                if !source_matches {
                    return Err(Error::validation(
                        "Sensor trigger contract does not match its release",
                    ));
                }
            }
            binding.event.validate()?;
            for (schema, contract) in [
                (&trigger.param_schema, &binding.parameters),
                (&trigger.out_schema, &binding.event.payload),
                (&trigger.event_config_schema, &binding.event.config),
            ] {
                match (schema, contract) {
                    (None, None) => {}
                    (Some(schema), Some(contract))
                        if crate::value_schema::digest(schema).map_err(|_| {
                            Error::validation("Invalid sensor trigger authored schema")
                        })? == contract.authored_digest => {}
                    _ => return Err(Error::validation("Sensor trigger schema digest mismatch")),
                }
            }
            if let Some(parameters) = &binding.parameters {
                if parameters.context != SchemaContext::FlatDocument
                    || matches!(parameters.source, SourceIdentity::Authoring { .. })
                {
                    return Err(Error::validation(
                        "Invalid sensor trigger parameter contract",
                    ));
                }
                CompiledContract::new(parameters)
                    .map_err(|_| Error::validation("Invalid sensor trigger parameter binding"))?;
            }
        }
        Ok(())
    }

    pub fn prepare_config(&self, value: &Value) -> Result<Value> {
        self.validate()?;
        let contract = CompiledContract::new(&self.config)
            .map_err(|_| Error::validation("Invalid sensor config contract"))?;
        // Defaults were selected before operator whole-field replacement. Delivery
        // must not recursively fill an overridden object from the schema again.
        contract
            .validate(value)
            .map_err(|_| Error::validation("Invalid sensor config fields"))?;
        Ok(value.clone())
    }
}
