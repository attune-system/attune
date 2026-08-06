use crate::repositories::cache::{validate_namespace_name, CacheNamespacePolicy};
use crate::Result;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CacheDefinitionOwnerType {
    Pack,
    Action,
    Sensor,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CacheDefinitionYaml {
    pub r#ref: String,
    pub namespace: String,
    pub owner_type: CacheDefinitionOwnerType,
    pub owner_ref: String,
    #[serde(default = "default_freshness_target_seconds")]
    pub freshness_target_seconds: i64,
    #[serde(default = "default_max_records_per_generation")]
    pub max_records_per_generation: i64,
    #[serde(default = "default_max_generation_bytes")]
    pub max_generation_bytes: i64,
    #[serde(default = "default_max_retained_bytes")]
    pub max_retained_bytes: i64,
    #[serde(default = "default_max_retained_generations")]
    pub max_retained_generations: i32,
    #[serde(default = "default_max_staging_generations")]
    pub max_staging_generations: i32,
}

impl CacheDefinitionYaml {
    pub(crate) fn policy(&self) -> CacheNamespacePolicy {
        CacheNamespacePolicy {
            freshness_target_seconds: self.freshness_target_seconds,
            max_records_per_generation: self.max_records_per_generation,
            max_generation_bytes: self.max_generation_bytes,
            max_retained_bytes: self.max_retained_bytes,
            max_retained_generations: self.max_retained_generations,
            max_staging_generations: self.max_staging_generations,
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate_namespace_name(&self.namespace)?;
        self.policy().validate()
    }
}

fn default_freshness_target_seconds() -> i64 {
    CacheNamespacePolicy::default().freshness_target_seconds
}

fn default_max_records_per_generation() -> i64 {
    CacheNamespacePolicy::default().max_records_per_generation
}

fn default_max_generation_bytes() -> i64 {
    CacheNamespacePolicy::default().max_generation_bytes
}

fn default_max_retained_bytes() -> i64 {
    CacheNamespacePolicy::default().max_retained_bytes
}

fn default_max_retained_generations() -> i32 {
    CacheNamespacePolicy::default().max_retained_generations
}

fn default_max_staging_generations() -> i32 {
    CacheNamespacePolicy::default().max_staging_generations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> CacheDefinitionYaml {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn uses_canonical_policy_defaults() {
        let definition =
            parse("ref: demo.users\nnamespace: users\nowner_type: pack\nowner_ref: demo\n");
        assert_eq!(definition.policy(), CacheNamespacePolicy::default());
        assert!(definition.validate().is_ok());
    }

    #[test]
    fn enforces_namespace_and_policy_constraints() {
        let bad_namespace =
            parse("ref: demo.users\nnamespace: Bad/Name\nowner_type: pack\nowner_ref: demo\n");
        assert!(bad_namespace.validate().is_err());

        let insufficient_retention = parse(
            "ref: demo.users\nnamespace: users\nowner_type: pack\nowner_ref: demo\nmax_retained_generations: 1\n",
        );
        assert!(insufficient_retention.validate().is_err());

        let zero_freshness = parse(
            "ref: demo.users\nnamespace: users\nowner_type: pack\nowner_ref: demo\nfreshness_target_seconds: 0\n",
        );
        assert!(zero_freshness.validate().is_ok());
    }
}
