//! Channel policy drives mirroring; private originals are never mirror input.

use attune_common::{
    models::OutputFormat,
    output_classification::{
        classify_action_output, output_channel_projections, OutputChannel,
        OutputChannelProjectionReason,
    },
    output_policy::{OutputVisibility, ResolvedOutputPolicy, StdoutPolicy},
    secret_provenance::SecretProvenance,
    value_schema::Contract,
};
use serde_json::{json, Value};

pub(super) struct ActionOutputMirrorInput<'a> {
    pub stdout: &'a str,
    pub stderr: &'a str,
    pub output_format: OutputFormat,
    pub output_policy: ResolvedOutputPolicy,
    pub out_schema: Option<&'a Contract>,
    pub stdout_truncated: bool,
    pub logs_incomplete: bool,
    pub mirror_stdout: bool,
    pub mirror_stderr: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SafeActionOutputMirror {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionOutputMirrorSuppression {
    IncompleteCapture,
    TruncatedCapture,
    OutputParseFailed,
    ResourceLimit,
    InvalidPolicy,
}

impl ActionOutputMirrorSuppression {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::IncompleteCapture => "incomplete_capture",
            Self::TruncatedCapture => "truncated_capture",
            Self::OutputParseFailed => "output_parse_failed",
            Self::ResourceLimit => "resource_limit",
            Self::InvalidPolicy => "invalid_policy",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ActionOutputMirrorDecision {
    Live,
    Delayed(SafeActionOutputMirror),
    Suppressed(ActionOutputMirrorSuppression),
}

pub(super) fn action_output_requires_delayed_mirror(policy: ResolvedOutputPolicy) -> bool {
    policy.stdout == StdoutPolicy::Sanitized
}

/// Uses the canonical classifier so result views and console projections cannot
/// disagree about secret fields, malformed values, or JSONL records.
pub(super) fn safe_action_output_mirror(
    input: ActionOutputMirrorInput<'_>,
) -> ActionOutputMirrorDecision {
    if input.output_policy.stdout == StdoutPolicy::Public
        && input.output_policy.stderr == OutputVisibility::Public
    {
        return ActionOutputMirrorDecision::Live;
    }
    let stderr = (input.mirror_stderr && input.output_policy.stderr == OutputVisibility::Public)
        .then(|| input.stderr.to_string());
    if input.output_policy.stdout != StdoutPolicy::Sanitized || !input.mirror_stdout {
        return ActionOutputMirrorDecision::Delayed(SafeActionOutputMirror {
            stdout: None,
            stderr,
        });
    }
    if input.logs_incomplete {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::IncompleteCapture,
        );
    }
    if input.stdout_truncated {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::TruncatedCapture,
        );
    }
    let classified = match classify_action_output(
        json!({"stdout":input.stdout}),
        input.out_schema,
        input.output_format,
        input.output_policy,
        1,
        &SecretProvenance::default(),
    ) {
        Ok(classified) => classified,
        Err(_) => {
            return ActionOutputMirrorDecision::Suppressed(
                ActionOutputMirrorSuppression::InvalidPolicy,
            )
        }
    };
    let projections = match output_channel_projections(&classified.public_result, 1) {
        Ok(projections) => projections,
        Err(_) => {
            return ActionOutputMirrorDecision::Suppressed(
                ActionOutputMirrorSuppression::InvalidPolicy,
            )
        }
    };
    if let Some(reason) = projections
        .get(&OutputChannel::Stdout)
        .and_then(|projection| projection.reason)
    {
        return ActionOutputMirrorDecision::Suppressed(match reason {
            OutputChannelProjectionReason::Truncated => {
                ActionOutputMirrorSuppression::TruncatedCapture
            }
            OutputChannelProjectionReason::Unparseable => {
                ActionOutputMirrorSuppression::OutputParseFailed
            }
            OutputChannelProjectionReason::ResourceLimit => {
                ActionOutputMirrorSuppression::ResourceLimit
            }
        });
    }
    match classified
        .public_result
        .get("stdout")
        .and_then(Value::as_str)
    {
        Some(stdout) => ActionOutputMirrorDecision::Delayed(SafeActionOutputMirror {
            stdout: Some(stdout.into()),
            stderr,
        }),
        None => {
            ActionOutputMirrorDecision::Suppressed(ActionOutputMirrorSuppression::OutputParseFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        output_policy::OutputPolicy,
        secret_values::is_redaction_marker,
        value_schema::{compile_inline, SchemaContext},
    };

    fn schema() -> Contract {
        compile_inline(
            "test.action",
            SchemaContext::FlatDocument,
            &json!({
                "token":{"type":"string","secret":true},"public":{"type":"string"}
            }),
        )
        .unwrap()
    }

    fn mirror(
        stdout: &str,
        stderr: &str,
        format: OutputFormat,
        schema: Option<&Contract>,
        policy: ResolvedOutputPolicy,
    ) -> ActionOutputMirrorDecision {
        safe_action_output_mirror(ActionOutputMirrorInput {
            stdout,
            stderr,
            output_format: format,
            output_policy: policy,
            out_schema: schema,
            stdout_truncated: false,
            logs_incomplete: false,
            mirror_stdout: true,
            mirror_stderr: true,
        })
    }

    #[test]
    fn public_channels_keep_live_mirroring_without_any_input_secret_check() {
        let policy = OutputPolicy::default()
            .resolve(None, OutputFormat::Text)
            .unwrap();
        assert!(!action_output_requires_delayed_mirror(policy));
        assert_eq!(
            mirror(
                "public summary",
                "public diagnostic",
                OutputFormat::Text,
                None,
                policy
            ),
            ActionOutputMirrorDecision::Live
        );
    }

    #[test]
    fn explicit_secret_channels_never_mirror_even_without_a_schema() {
        let policy = ResolvedOutputPolicy {
            stdout: StdoutPolicy::Secret,
            stderr: OutputVisibility::Secret,
            error: OutputVisibility::Public,
        };
        assert!(!action_output_requires_delayed_mirror(policy));
        assert_eq!(
            mirror(
                "private-stdout",
                "private-stderr",
                OutputFormat::Text,
                None,
                policy
            ),
            ActionOutputMirrorDecision::Delayed(SafeActionOutputMirror {
                stdout: None,
                stderr: None
            })
        );
    }

    #[test]
    fn public_stderr_is_independent_of_private_stdout() {
        let policy = ResolvedOutputPolicy {
            stdout: StdoutPolicy::Secret,
            stderr: OutputVisibility::Public,
            error: OutputVisibility::Public,
        };
        assert_eq!(
            mirror(
                "private",
                "public diagnostic",
                OutputFormat::Text,
                None,
                policy
            ),
            ActionOutputMirrorDecision::Delayed(SafeActionOutputMirror {
                stdout: None,
                stderr: Some("public diagnostic".into())
            })
        );
    }

    #[test]
    fn sanitized_stdout_uses_canonical_markers_and_omits_private_stderr() {
        let schema = schema();
        let policy = OutputPolicy::default()
            .resolve(Some(&schema), OutputFormat::Json)
            .unwrap();
        assert!(action_output_requires_delayed_mirror(policy));
        let ActionOutputMirrorDecision::Delayed(content) = mirror(
            r#"{"token":"private-token","public":"visible"}"#,
            "private-token",
            OutputFormat::Json,
            Some(&schema),
            policy,
        ) else {
            panic!("expected sanitized mirror")
        };
        let stdout = content.stdout.unwrap();
        assert!(!stdout.contains("private-token"));
        let data: Value = serde_json::from_str(&stdout).unwrap();
        assert!(is_redaction_marker(&data["token"]));
        assert_eq!(data["public"], "visible");
        assert!(content.stderr.is_none());
    }

    #[test]
    fn explicit_public_stderr_does_not_inherit_output_field_secrecy() {
        let schema = schema();
        let policy = OutputPolicy {
            stderr: Some(OutputVisibility::Public),
            ..Default::default()
        }
        .resolve(Some(&schema), OutputFormat::Json)
        .unwrap();
        let ActionOutputMirrorDecision::Delayed(content) = mirror(
            r#"{"token":"private-token"}"#,
            "author-declared public stderr",
            OutputFormat::Json,
            Some(&schema),
            policy,
        ) else {
            panic!("expected sanitized mirror")
        };
        assert_eq!(
            content.stderr.as_deref(),
            Some("author-declared public stderr")
        );
    }

    #[test]
    fn leading_debug_lines_never_enter_the_sanitized_projection() {
        let schema = schema();
        let policy = OutputPolicy::default()
            .resolve(Some(&schema), OutputFormat::Json)
            .unwrap();
        let ActionOutputMirrorDecision::Delayed(content) = mirror(
            "unclassified private prefix\n{\"token\":\"private-token\",\"public\":\"visible\"}",
            "",
            OutputFormat::Json,
            Some(&schema),
            policy,
        ) else {
            panic!("expected sanitized mirror")
        };
        let stdout = content.stdout.unwrap();
        assert!(!stdout.contains("prefix"));
        assert!(!stdout.contains("private-token"));
        assert!(stdout.contains("visible"));
    }

    #[test]
    fn jsonl_classifies_each_record_before_any_mirroring() {
        let schema = schema();
        let policy = OutputPolicy::default()
            .resolve(Some(&schema), OutputFormat::Jsonl)
            .unwrap();
        let ActionOutputMirrorDecision::Delayed(content) = mirror(
            "{\"token\":\"first\",\"public\":\"one\"}\n{\"token\":\"second\",\"public\":\"two\"}",
            "",
            OutputFormat::Jsonl,
            Some(&schema),
            policy,
        ) else {
            panic!("expected sanitized mirror")
        };
        let stdout = content.stdout.unwrap();
        assert_eq!(stdout.lines().count(), 2);
        assert!(!stdout.contains("first"));
        assert!(!stdout.contains("second"));
    }

    #[test]
    fn malformed_middle_jsonl_record_suppresses_opaque_raw_bytes() {
        let schema = schema();
        let policy = OutputPolicy::default()
            .resolve(Some(&schema), OutputFormat::Jsonl)
            .unwrap();
        assert_eq!(
            mirror(
                "{\"token\":\"one\"}\nmalformed-private-debug\n{\"token\":\"two\"}",
                "",
                OutputFormat::Jsonl,
                Some(&schema),
                policy
            ),
            ActionOutputMirrorDecision::Suppressed(
                ActionOutputMirrorSuppression::OutputParseFailed
            )
        );
    }

    #[test]
    fn invalid_public_output_fields_do_not_suppress_safe_projection() {
        let schema = schema();
        let policy = OutputPolicy::default()
            .resolve(Some(&schema), OutputFormat::Json)
            .unwrap();
        let ActionOutputMirrorDecision::Delayed(content) = mirror(
            r#"{"token":99,"public":37}"#,
            "",
            OutputFormat::Json,
            Some(&schema),
            policy,
        ) else {
            panic!("expected sanitized mirror")
        };
        let stdout = content.stdout.unwrap();
        let data: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(data["public"], 37);
        assert!(is_redaction_marker(&data["token"]));
    }

    #[test]
    fn absent_secret_fields_do_not_suppress_public_output() {
        let schema = schema();
        let policy = OutputPolicy::default()
            .resolve(Some(&schema), OutputFormat::Json)
            .unwrap();
        let ActionOutputMirrorDecision::Delayed(content) = mirror(
            r#"{"public":"visible"}"#,
            "",
            OutputFormat::Json,
            Some(&schema),
            policy,
        ) else {
            panic!("expected sanitized mirror")
        };
        assert_eq!(content.stdout.as_deref(), Some(r#"{"public":"visible"}"#));
    }

    #[test]
    fn truncated_or_incomplete_stdout_never_mirrors_raw_bytes() {
        let schema = schema();
        let policy = OutputPolicy::default()
            .resolve(Some(&schema), OutputFormat::Json)
            .unwrap();
        for (truncated, incomplete, expected) in [
            (true, false, ActionOutputMirrorSuppression::TruncatedCapture),
            (
                false,
                true,
                ActionOutputMirrorSuppression::IncompleteCapture,
            ),
        ] {
            assert_eq!(
                safe_action_output_mirror(ActionOutputMirrorInput {
                    stdout: r#"{"token":"private"}"#,
                    stderr: "",
                    output_format: OutputFormat::Json,
                    output_policy: policy,
                    out_schema: Some(&schema),
                    stdout_truncated: truncated,
                    logs_incomplete: incomplete,
                    mirror_stdout: true,
                    mirror_stderr: false,
                }),
                ActionOutputMirrorDecision::Suppressed(expected)
            );
        }
    }

    #[test]
    fn only_requested_public_projections_are_returned() {
        let schema = schema();
        let policy = OutputPolicy {
            stderr: Some(OutputVisibility::Public),
            ..Default::default()
        }
        .resolve(Some(&schema), OutputFormat::Json)
        .unwrap();
        let ActionOutputMirrorDecision::Delayed(content) =
            safe_action_output_mirror(ActionOutputMirrorInput {
                stdout: r#"{"token":"private"}"#,
                stderr: "public",
                output_format: OutputFormat::Json,
                output_policy: policy,
                out_schema: Some(&schema),
                stdout_truncated: false,
                logs_incomplete: false,
                mirror_stdout: false,
                mirror_stderr: true,
            })
        else {
            panic!("expected separate stderr projection")
        };
        assert!(content.stdout.is_none());
        assert_eq!(content.stderr.as_deref(), Some("public"));
    }
}
