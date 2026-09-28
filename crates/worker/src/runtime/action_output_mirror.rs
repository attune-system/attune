use attune_common::models::OutputFormat;
use attune_common::secret_values::{
    is_redaction_marker, redact_secret_parameters, secret_paths_from_schema,
};
use serde_json::Value;
use std::collections::BTreeSet;

const REDACTION: &str = "[REDACTED]";

pub(super) struct ActionOutputMirrorInput<'a> {
    pub stdout: &'a str,
    pub stderr: &'a str,
    pub output_format: OutputFormat,
    pub out_schema: Option<&'a Value>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
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
    TextOutputWithSecretSchema,
    IncompleteCapture,
    TruncatedCapture,
    OutputParseFailed,
    SecretValuesUnavailable,
}

impl ActionOutputMirrorSuppression {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::TextOutputWithSecretSchema => "text_output_with_secret_schema",
            Self::IncompleteCapture => "incomplete_capture",
            Self::TruncatedCapture => "truncated_capture",
            Self::OutputParseFailed => "output_parse_failed",
            Self::SecretValuesUnavailable => "secret_values_unavailable",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ActionOutputMirrorDecision {
    Live,
    Delayed(SafeActionOutputMirror),
    Suppressed(ActionOutputMirrorSuppression),
}

pub(super) fn action_output_requires_delayed_mirror(out_schema: Option<&Value>) -> bool {
    !secret_paths_from_schema(out_schema).is_empty()
}

pub(super) fn safe_action_output_mirror(
    input: ActionOutputMirrorInput<'_>,
) -> ActionOutputMirrorDecision {
    let secret_paths = secret_paths_from_schema(input.out_schema);
    if secret_paths.is_empty() {
        return ActionOutputMirrorDecision::Live;
    }
    if !input.mirror_stdout && !input.mirror_stderr {
        return ActionOutputMirrorDecision::Delayed(SafeActionOutputMirror {
            stdout: None,
            stderr: None,
        });
    }
    if input.output_format == OutputFormat::Text {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::TextOutputWithSecretSchema,
        );
    }
    if input.logs_incomplete {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::IncompleteCapture,
        );
    }
    if input.stdout_truncated || input.stderr_truncated {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::TruncatedCapture,
        );
    }

    let Some(mut parsed) = parse_completed_stdout(input.stdout, input.output_format) else {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::OutputParseFailed,
        );
    };
    let mut found_paths = BTreeSet::new();
    for record in parsed.records_mut() {
        let (mut redacted, secrets) = redact_secret_parameters(record.clone(), input.out_schema);
        replace_redaction_markers(&mut redacted);
        *record = redacted;
        for secret in secrets {
            found_paths.insert(secret.json_path);
        }
    }
    if secret_paths
        .iter()
        .any(|path| !found_paths.contains(path.as_str()))
    {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::SecretValuesUnavailable,
        );
    }

    let Some(stdout) = serialize_redacted_stdout(parsed) else {
        return ActionOutputMirrorDecision::Suppressed(
            ActionOutputMirrorSuppression::OutputParseFailed,
        );
    };
    ActionOutputMirrorDecision::Delayed(SafeActionOutputMirror {
        stdout: input.mirror_stdout.then_some(stdout),
        stderr: input.mirror_stderr.then(|| redact_diagnostic(input.stderr)),
    })
}

enum ParsedStdout {
    Json {
        prefix: Option<String>,
        value: Value,
    },
    Yaml(Value),
    Jsonl(Vec<Value>),
}

impl ParsedStdout {
    fn records_mut(&mut self) -> Vec<&mut Value> {
        match self {
            Self::Json { value, .. } | Self::Yaml(value) => vec![value],
            Self::Jsonl(records) => records.iter_mut().collect(),
        }
    }
}

fn parse_completed_stdout(stdout: &str, format: OutputFormat) -> Option<ParsedStdout> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    match format {
        OutputFormat::Text => None,
        OutputFormat::Json => serde_json::from_str(trimmed)
            .ok()
            .map(|value| ParsedStdout::Json {
                prefix: None,
                value,
            })
            .or_else(|| {
                let trimmed_end = stdout.trim_end();
                let (prefix, final_line) = trimmed_end.rsplit_once('\n')?;
                serde_json::from_str(final_line.trim())
                    .ok()
                    .map(|value| ParsedStdout::Json {
                        prefix: Some(prefix.to_string()),
                        value,
                    })
            }),
        OutputFormat::Yaml => serde_yaml_ng::from_str(trimmed)
            .ok()
            .map(ParsedStdout::Yaml),
        OutputFormat::Jsonl => trimmed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<Vec<Value>, _>>()
            .ok()
            .filter(|records| !records.is_empty())
            .map(ParsedStdout::Jsonl),
    }
}

fn serialize_redacted_stdout(parsed: ParsedStdout) -> Option<String> {
    match parsed {
        ParsedStdout::Json { prefix, value } => {
            let result = serde_json::to_string(&value).ok()?;
            match prefix {
                Some(prefix) if !prefix.is_empty() => Some(format!("{REDACTION}\n{result}")),
                None => Some(result),
                Some(_) => Some(result),
            }
        }
        ParsedStdout::Yaml(value) => serde_yaml_ng::to_string(&value).ok(),
        ParsedStdout::Jsonl(records) => records
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .map(|lines| lines.join("\n")),
    }
}

fn redact_diagnostic(content: &str) -> String {
    if content.is_empty() {
        String::new()
    } else if content.ends_with('\n') {
        format!("{REDACTION}\n")
    } else {
        REDACTION.to_string()
    }
}

fn replace_redaction_markers(value: &mut Value) {
    if is_redaction_marker(value) {
        *value = Value::String(REDACTION.to_string());
        return;
    }
    match value {
        Value::Array(items) => items.iter_mut().for_each(replace_redaction_markers),
        Value::Object(object) => object.values_mut().for_each(replace_redaction_markers),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::models::OutputFormat;
    use serde_json::json;

    #[test]
    fn schema_secret_printed_to_both_streams_is_only_mirrored_redacted() {
        let secret = "unique-action-output-secret";
        let schema = json!({
            "token": {"type": "string", "secret": true}
        });

        let decision = safe_action_output_mirror(ActionOutputMirrorInput {
            stdout: &format!("{{\"token\":\"{secret}\",\"status\":\"ok\"}}\n"),
            stderr: &format!("debug token={secret}\n"),
            output_format: OutputFormat::Json,
            out_schema: Some(&schema),
            stdout_truncated: false,
            stderr_truncated: false,
            logs_incomplete: false,
            mirror_stdout: true,
            mirror_stderr: true,
        });

        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("secret output must use delayed mirroring");
        };
        let stdout = content.stdout.expect("stdout mirror");
        let stderr = content.stderr.expect("stderr mirror");
        assert!(!stdout.contains(secret));
        assert!(!stderr.contains(secret));
        assert!(stdout.contains("[REDACTED]"));
        assert!(stderr.contains("[REDACTED]"));
    }

    fn mirror(
        stdout: &str,
        stderr: &str,
        format: OutputFormat,
        schema: &Value,
        mirror_stdout: bool,
        mirror_stderr: bool,
    ) -> ActionOutputMirrorDecision {
        safe_action_output_mirror(ActionOutputMirrorInput {
            stdout,
            stderr,
            output_format: format,
            out_schema: Some(schema),
            stdout_truncated: false,
            stderr_truncated: false,
            logs_incomplete: false,
            mirror_stdout,
            mirror_stderr,
        })
    }

    #[test]
    fn secret_streams_are_independently_enabled() {
        let schema = json!({"secret": {"type": "string", "secret": true}});
        for (stdout_enabled, stderr_enabled) in
            [(true, false), (false, true), (true, true), (false, false)]
        {
            let ActionOutputMirrorDecision::Delayed(content) = mirror(
                r#"{"secret":"value"}"#,
                "value",
                OutputFormat::Json,
                &schema,
                stdout_enabled,
                stderr_enabled,
            ) else {
                panic!("expected delayed mirror");
            };
            assert_eq!(content.stdout.is_some(), stdout_enabled);
            assert_eq!(content.stderr.is_some(), stderr_enabled);
        }
    }

    #[test]
    fn actions_without_secret_paths_keep_live_mirroring() {
        let decision = mirror(
            r#"{"value":"public"}"#,
            "public",
            OutputFormat::Json,
            &json!({"value": {"type": "string"}}),
            true,
            true,
        );
        assert_eq!(decision, ActionOutputMirrorDecision::Live);
    }

    #[test]
    fn json_final_line_fallback_supplies_secret_values() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        let decision = mirror(
            "public log\n{\"token\":\"fallback-secret\"}\n",
            "fallback-secret",
            OutputFormat::Json,
            &schema,
            true,
            true,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert!(!content.stdout.unwrap().contains("fallback-secret"));
        assert!(!content.stderr.unwrap().contains("fallback-secret"));
    }

    #[test]
    fn json_reserializes_unicode_escapes_and_exponent_numbers_after_redaction() {
        let schema = json!({
            "token": {"type": "string", "secret": true},
            "count": {"type": "number", "secret": true}
        });
        let decision = mirror(
            r#"{"token":"\u0073\u0065\u0063\u0072\u0065\u0074","count":1e3,"public":"ok"}"#,
            "",
            OutputFormat::Json,
            &schema,
            true,
            false,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        let stdout = content.stdout.unwrap();
        assert!(!stdout.contains("secret"));
        assert!(!stdout.contains("1000"));
        assert!(!stdout.contains("1e3"));
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).unwrap()["public"],
            "ok"
        );
    }

    #[test]
    fn formatted_object_and_array_secrets_are_structurally_redacted() {
        let schema = json!({
            "credentials": {"type": "object", "secret": true},
            "codes": {"type": "array", "secret": true}
        });
        let decision = mirror(
            r#"{
  "credentials": {
    "token": "object-secret"
  },
  "codes": [
    1e3,
    "array-secret"
  ],
  "public": true
}"#,
            "",
            OutputFormat::Json,
            &schema,
            true,
            false,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        let stdout = content.stdout.unwrap();
        assert!(!stdout.contains("object-secret"));
        assert!(!stdout.contains("array-secret"));
        assert!(!stdout.contains("1000"));
        let mirrored: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(mirrored["credentials"], REDACTION);
        assert_eq!(mirrored["codes"], REDACTION);
        assert_eq!(mirrored["public"], true);
    }

    #[test]
    fn json_prefix_and_stderr_are_redacted_wholesale() {
        let schema = json!({
            "token": {"type": "string", "secret": true},
            "count": {"type": "number", "secret": true}
        });
        let stdout = concat!(
            r#"{"message":"saw snowman \u2603","count":1e3}"#,
            "\n\n",
            r#"{"token":"snowman \u2603","count":1e3}"#,
        );
        let stderr = concat!(
            r#"{"message":"failed for snowman \u2603","count":1e3}"#,
            "\npublic line\n",
        );
        let decision = mirror(stdout, stderr, OutputFormat::Json, &schema, true, true);
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert_eq!(
            content.stdout.as_deref(),
            Some("[REDACTED]\n{\"count\":\"[REDACTED]\",\"token\":\"[REDACTED]\"}")
        );
        assert_eq!(content.stderr.as_deref(), Some("[REDACTED]\n"));
    }

    #[test]
    fn alternate_json_numbers_in_diagnostics_are_redacted_wholesale() {
        let schema = json!({"count": {"type": "number", "secret": true}});
        let stdout = concat!(
            r#"{"integer":1e3,"decimal":1000.0,"public":7}"#,
            "\n",
            r#"{"count":1000}"#,
        );
        let stderr = r#"{"upper":1E+3,"decimal":1000.0,"public":7}"#;

        let decision = mirror(stdout, stderr, OutputFormat::Json, &schema, true, true);
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert_eq!(content.stderr.as_deref(), Some(REDACTION));
        assert_eq!(
            content.stdout.as_deref(),
            Some("[REDACTED]\n{\"count\":\"[REDACTED]\"}")
        );
    }

    #[test]
    fn object_secret_keys_are_redacted_in_stderr_and_json_prefix() {
        let schema = json!({"credentials": {"type": "object", "secret": true}});
        let stdout = concat!(
            r#"diagnostic api_key=failed {"api_key":"named field"}"#,
            "\n",
            r#"{"credentials":{"api_key":"object-secret"}}"#,
        );
        let stderr = r#"failed api_key {"api_key":"named field"}"#;

        let decision = mirror(stdout, stderr, OutputFormat::Json, &schema, true, true);
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert_eq!(content.stderr.as_deref(), Some(REDACTION));
        assert_eq!(
            content.stdout.as_deref(),
            Some("[REDACTED]\n{\"credentials\":\"[REDACTED]\"}")
        );
    }

    #[test]
    fn jsonl_applies_the_output_schema_to_every_record() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        let decision = mirror(
            "{\"token\":\"first-secret\"}\n{\"token\":\"second-secret\"}\n",
            "first-secret second-secret",
            OutputFormat::Jsonl,
            &schema,
            true,
            true,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        let combined = format!("{}{}", content.stdout.unwrap(), content.stderr.unwrap());
        assert!(!combined.contains("first-secret"));
        assert!(!combined.contains("second-secret"));
        assert_eq!(combined.matches(REDACTION).count(), 3);
    }

    #[test]
    fn yaml_output_supplies_secret_values() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        let decision = mirror(
            "token: yaml-secret\nstatus: ok\n",
            "yaml-secret",
            OutputFormat::Yaml,
            &schema,
            true,
            true,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert!(!content.stdout.unwrap().contains("yaml-secret"));
        assert!(!content.stderr.unwrap().contains("yaml-secret"));
    }

    #[test]
    fn yaml_alternate_scalar_forms_are_redacted_before_reserialization() {
        let schema = json!({
            "hex": {"type": "number", "secret": true},
            "underscored": {"type": "number", "secret": true}
        });
        let decision = mirror(
            "hex: 0x2A\nunderscored: 1_000\npublic: ok\n",
            "hex=42, underscored=1000, public diagnostic\n",
            OutputFormat::Yaml,
            &schema,
            true,
            true,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        let stdout = content.stdout.unwrap();
        assert!(!stdout.contains("0x2A"));
        assert!(!stdout.contains("1_000"));
        assert!(!stdout.contains("1000"));
        let mirrored: Value = serde_yaml_ng::from_str(&stdout).unwrap();
        assert_eq!(mirrored["hex"], REDACTION);
        assert_eq!(mirrored["underscored"], REDACTION);
        assert_eq!(mirrored["public"], "ok");
        assert_eq!(content.stderr.as_deref(), Some("[REDACTED]\n"));
    }

    #[test]
    fn parse_failure_and_text_output_fail_closed() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        assert_eq!(
            mirror(
                "not json",
                "secret",
                OutputFormat::Json,
                &schema,
                true,
                true,
            ),
            ActionOutputMirrorDecision::Suppressed(
                ActionOutputMirrorSuppression::OutputParseFailed
            )
        );
        assert_eq!(
            mirror("secret", "secret", OutputFormat::Text, &schema, true, true,),
            ActionOutputMirrorDecision::Suppressed(
                ActionOutputMirrorSuppression::TextOutputWithSecretSchema
            )
        );
    }

    #[test]
    fn truncated_or_incomplete_capture_fails_closed() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        let base = || ActionOutputMirrorInput {
            stdout: r#"{"token":"secret"}"#,
            stderr: "secret",
            output_format: OutputFormat::Json,
            out_schema: Some(&schema),
            stdout_truncated: false,
            stderr_truncated: false,
            logs_incomplete: false,
            mirror_stdout: true,
            mirror_stderr: true,
        };
        assert_eq!(
            safe_action_output_mirror(ActionOutputMirrorInput {
                stdout_truncated: true,
                ..base()
            }),
            ActionOutputMirrorDecision::Suppressed(ActionOutputMirrorSuppression::TruncatedCapture)
        );
        assert_eq!(
            safe_action_output_mirror(ActionOutputMirrorInput {
                logs_incomplete: true,
                ..base()
            }),
            ActionOutputMirrorDecision::Suppressed(
                ActionOutputMirrorSuppression::IncompleteCapture
            )
        );
    }

    #[test]
    fn arbitrary_chunked_stderr_is_redacted_once() {
        let schema = json!({
            "short": {"type": "string", "secret": true},
            "long": {"type": "string", "secret": true}
        });
        let stdout_chunks = [r#"{"short":"overlap","long":"overlap-"#, r#"secret"}"#];
        let stderr_chunks = ["overlap-", "secret overlap"];
        let decision = mirror(
            &stdout_chunks.concat(),
            &stderr_chunks.concat(),
            OutputFormat::Json,
            &schema,
            true,
            true,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert_eq!(content.stderr.as_deref(), Some(REDACTION));
    }

    #[test]
    fn non_string_secret_diagnostics_are_redacted_wholesale() {
        let schema = json!({
            "number": {"type": "number", "secret": true},
            "flag": {"type": "boolean", "secret": true},
            "object": {"type": "object", "secret": true},
            "array": {"type": "array", "secret": true}
        });
        let stdout = r#"{"number":7319,"flag":true,"object":{"key":"object-leaf"},"array":["array-leaf",8821]}"#;
        let stderr =
            r#"7319 true {"key":"object-leaf"} object-leaf ["array-leaf",8821] array-leaf 8821"#;
        let decision = mirror(stdout, stderr, OutputFormat::Json, &schema, true, true);
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert_eq!(content.stderr.as_deref(), Some(REDACTION));
        let stdout: Value = serde_json::from_str(content.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(stdout["number"], REDACTION);
        assert_eq!(stdout["flag"], REDACTION);
        assert_eq!(stdout["object"], REDACTION);
        assert_eq!(stdout["array"], REDACTION);
    }

    #[test]
    fn non_empty_diagnostics_are_redacted_even_when_secret_value_is_empty() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        let decision = mirror(
            r#"{"token":"","status":"ok"}"#,
            "public stderr",
            OutputFormat::Json,
            &schema,
            true,
            true,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert_eq!(content.stderr.as_deref(), Some(REDACTION));
    }

    #[test]
    fn empty_diagnostics_remain_empty() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        let decision = mirror(
            r#"{"token":"secret"}"#,
            "",
            OutputFormat::Json,
            &schema,
            true,
            true,
        );
        let ActionOutputMirrorDecision::Delayed(content) = decision else {
            panic!("expected delayed mirror");
        };
        assert_eq!(content.stderr.as_deref(), Some(""));
    }

    #[test]
    fn unavailable_secret_fields_fail_closed() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        assert_eq!(
            mirror(
                r#"{"status":"ok"}"#,
                "diagnostic",
                OutputFormat::Json,
                &schema,
                true,
                true,
            ),
            ActionOutputMirrorDecision::Suppressed(
                ActionOutputMirrorSuppression::SecretValuesUnavailable
            )
        );
    }
}
