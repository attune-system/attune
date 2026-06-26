use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum QuerySafetyError {
    #[error("reference value has invalid format")]
    InvalidRefFormat,
    #[error("path `{0}` is not allow-listed")]
    PathNotAllowed(String),
    #[error("limit must be at least 1")]
    InvalidLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypedBindValue {
    Text(String),
    Int64(i64),
    Bool(bool),
    Timestamp(DateTime<Utc>),
    TextArray(Vec<String>),
    Int64Array(Vec<i64>),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeQueryBindings {
    values: BTreeMap<String, TypedBindValue>,
}

impl SafeQueryBindings {
    pub fn new() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: impl Into<String>, value: TypedBindValue) {
        self.values.insert(key.into(), value);
    }

    pub fn get(&self, key: &str) -> Option<&TypedBindValue> {
        self.values.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &TypedBindValue)> {
        self.values.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeRef(String);

impl SafeRef {
    pub fn parse(value: &str) -> Result<Self, QuerySafetyError> {
        if value.is_empty() || value.len() > 128 {
            return Err(QuerySafetyError::InvalidRefFormat);
        }
        let mut chars = value.chars();
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
            _ => return Err(QuerySafetyError::InvalidRefFormat),
        }

        if value.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        }) {
            Ok(Self(value.to_string()))
        } else {
            Err(QuerySafetyError::InvalidRefFormat)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedLimit {
    pub value: i64,
    pub max: i64,
}

impl BoundedLimit {
    pub fn new(requested: Option<i64>, default: i64, max: i64) -> Result<Self, QuerySafetyError> {
        let mut value = requested.unwrap_or(default);
        if value < 1 {
            return Err(QuerySafetyError::InvalidLimit);
        }
        if value > max {
            value = max;
        }
        Ok(Self { value, max })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionResultPathAllowList {
    allowed_paths: BTreeSet<String>,
}

impl ActionResultPathAllowList {
    pub fn new<I, S>(allowed_paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed_paths: allowed_paths.into_iter().map(Into::into).collect(),
        }
    }

    pub fn require_allowed(&self, path: &str) -> Result<(), QuerySafetyError> {
        if self.allowed_paths.contains(path) {
            Ok(())
        } else {
            Err(QuerySafetyError::PathNotAllowed(path.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        ActionResultPathAllowList, BoundedLimit, QuerySafetyError, SafeQueryBindings, SafeRef,
        TypedBindValue,
    };

    #[test]
    fn safe_ref_accepts_valid_attune_refs() {
        let parsed = SafeRef::parse("core.execution_count").expect("valid ref");
        assert_eq!(parsed.as_str(), "core.execution_count");
    }

    #[test]
    fn safe_ref_rejects_injection_shape_chars() {
        let err = SafeRef::parse("core.exec;drop table execution");
        assert_eq!(err, Err(QuerySafetyError::InvalidRefFormat));
    }

    #[test]
    fn allow_list_rejects_unknown_path() {
        let allow_list = ActionResultPathAllowList::new(["summary.status", "metrics.p95_ms"]);
        let err = allow_list.require_allowed("details.sql");
        assert_eq!(
            err,
            Err(QuerySafetyError::PathNotAllowed("details.sql".to_string()))
        );
    }

    #[test]
    fn bounded_limit_clamps_to_server_max() {
        let limit = BoundedLimit::new(Some(10_000), 100, 1_000).expect("limit should be valid");
        assert_eq!(limit.value, 1_000);
    }

    #[test]
    fn typed_bindings_keep_parameter_types() {
        let mut bindings = SafeQueryBindings::new();
        bindings.insert("since", TypedBindValue::Timestamp(Utc::now()));
        bindings.insert("pack_ref", TypedBindValue::Text("core".to_string()));
        bindings.insert("limit", TypedBindValue::Int64(200));

        assert!(matches!(
            bindings.get("pack_ref"),
            Some(TypedBindValue::Text(value)) if value == "core"
        ));
        assert!(matches!(
            bindings.get("limit"),
            Some(TypedBindValue::Int64(200))
        ));
    }
}
