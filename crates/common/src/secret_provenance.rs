//! Immutable source bindings used for secret disclosure decisions.

use serde::{Deserialize, Serialize};

use crate::models::{Id, OwnerType};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyOrigin {
    pub key_id: Id,
    pub key_ref: String,
    pub owner_type: OwnerType,
    pub owner_identity: Option<Id>,
    pub owner_ref: Option<String>,
    pub encrypted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateOrigin {
    pub component_type: String,
    pub component_ref: String,
    pub input_path: String,
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretOrigin {
    Key(KeyOrigin),
    Entity {
        entity_type: String,
        entity_id: Id,
        path: String,
    },
    Local {
        source_kind: String,
        source_ref: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretProvenance {
    pub origins: Vec<SecretOrigin>,
    pub templates: Vec<TemplateOrigin>,
}

impl SecretProvenance {
    pub fn key(origin: KeyOrigin) -> Self {
        Self {
            origins: vec![SecretOrigin::Key(origin)],
            templates: Vec::new(),
        }
    }

    pub fn merge(&mut self, other: Self) {
        for origin in other.origins {
            if !self.origins.contains(&origin) {
                self.origins.push(origin);
            }
        }
        for template in other.templates {
            if !self.templates.contains(&template) {
                self.templates.push(template);
            }
        }
    }
}
