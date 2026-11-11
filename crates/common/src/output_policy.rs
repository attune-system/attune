//! Author-controlled output classification, independent of input provenance.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    models::OutputFormat, value_schema::CompiledContract, value_schema::Contract, Error, Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputVisibility {
    Public,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StdoutPolicy {
    Public,
    Secret,
    Sanitized,
}

/// Optional authoring choices. Resolve against the stored output contract once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<StdoutPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<OutputVisibility>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OutputVisibility>,
}

/// Concrete policy captured by an execution's immutable binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedOutputPolicy {
    pub stdout: StdoutPolicy,
    pub stderr: OutputVisibility,
    pub error: OutputVisibility,
}

impl Default for ResolvedOutputPolicy {
    fn default() -> Self {
        Self {
            stdout: StdoutPolicy::Public,
            stderr: OutputVisibility::Public,
            error: OutputVisibility::Public,
        }
    }
}

impl OutputPolicy {
    pub fn resolve(
        &self,
        output: Option<&Contract>,
        format: OutputFormat,
    ) -> Result<ResolvedOutputPolicy> {
        let sensitive = output
            .map(CompiledContract::new)
            .transpose()
            .map_err(|_| Error::validation("Invalid output classification contract"))?
            .is_some_and(|contract| contract.declares_secrets());
        self.resolve_sensitive(sensitive, format)
    }

    pub fn resolve_sensitive(
        &self,
        sensitive: bool,
        format: OutputFormat,
    ) -> Result<ResolvedOutputPolicy> {
        let structured = format != OutputFormat::Text;
        let stdout = self.stdout.unwrap_or(if sensitive && structured {
            StdoutPolicy::Sanitized
        } else if sensitive {
            StdoutPolicy::Secret
        } else {
            StdoutPolicy::Public
        });
        if stdout == StdoutPolicy::Sanitized && !structured {
            return Err(Error::validation(
                "Sanitized stdout requires a structured output format",
            ));
        }
        if stdout == StdoutPolicy::Public && sensitive {
            return Err(Error::validation(
                "Public stdout would expose schema-secret output fields",
            ));
        }
        let diagnostic_default = if sensitive {
            OutputVisibility::Secret
        } else {
            OutputVisibility::Public
        };
        Ok(ResolvedOutputPolicy {
            stdout,
            stderr: self.stderr.unwrap_or(diagnostic_default),
            error: self.error.unwrap_or(diagnostic_default),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_defaults_do_not_need_input_or_runtime_authority() {
        let policy = OutputPolicy::default()
            .resolve(None, OutputFormat::Text)
            .unwrap();
        assert_eq!(policy.stdout, StdoutPolicy::Public);
        assert_eq!(policy.stderr, OutputVisibility::Public);
        assert_eq!(policy.error, OutputVisibility::Public);
    }

    #[test]
    fn sensitive_structured_output_defaults_to_sanitized_stdout() {
        let policy = OutputPolicy::default()
            .resolve_sensitive(true, OutputFormat::Json)
            .unwrap();
        assert_eq!(policy.stdout, StdoutPolicy::Sanitized);
        assert_eq!(policy.stderr, OutputVisibility::Secret);
    }

    #[test]
    fn rejects_unusable_or_leaking_stdout_policies() {
        let sanitized = OutputPolicy {
            stdout: Some(StdoutPolicy::Sanitized),
            ..Default::default()
        };
        assert!(sanitized.resolve(None, OutputFormat::Text).is_err());
        let public = OutputPolicy {
            stdout: Some(StdoutPolicy::Public),
            ..Default::default()
        };
        assert!(public.resolve_sensitive(true, OutputFormat::Json).is_err());
    }
}
