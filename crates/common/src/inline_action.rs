//! Pure legacy inline shell rendering, shared by local and private guest paths.
//! Parameters become script-local variables, never exported child environment.
use std::collections::HashMap;

pub fn shell_wrapper(
    parameters: &HashMap<String, serde_json::Value>,
    code: &str,
) -> serde_json::Result<String> {
    let mut script = String::from("#!/bin/bash\nset -e\n\n# Action parameters\n");
    for (key, value) in parameters {
        let value = match value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::Bool(value) => value.to_string(),
            _ => serde_json::to_string(value)?,
        };
        let escaped = value.replace('\'', "'\\''");
        let mut identifier = String::with_capacity(key.len());
        for (index, ch) in key.chars().enumerate() {
            if (ch == '_' || ch.is_ascii_alphanumeric()) && !(index == 0 && ch.is_ascii_digit()) {
                identifier.push(ch);
            } else {
                identifier.push('_');
            }
        }
        if identifier.is_empty() {
            identifier.push('_');
        }
        script.push_str(&format!(
            "PARAM_{}='{}'\n",
            identifier.to_uppercase(),
            escaped
        ));
        script.push_str(&format!("{}='{}'\n", identifier, escaped));
    }
    script.push_str("\n# Action code\n");
    script.push_str(code);
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_parameters_remain_quoted_local_variables_and_code_is_unchanged() {
        let parameters = HashMap::from([
            (
                "9-quote".into(),
                serde_json::json!("'$(touch /tmp/marker)\nend"),
            ),
            ("".into(), serde_json::json!({"array":[true,null]})),
        ]);
        let code = "printf '%s' \"$PARAM___QUOTE\"\n";
        let script = shell_wrapper(&parameters, code).unwrap();
        assert!(script.starts_with("#!/bin/bash\nset -e\n"));
        assert!(script.contains("PARAM___QUOTE=''\\''$(touch /tmp/marker)\nend'\n"));
        assert!(script.contains("__quote=''\\''$(touch /tmp/marker)\nend'\n"));
        assert!(script.contains("_='{\"array\":[true,null]}'\n"));
        assert!(!script.contains("export "));
        assert!(script.ends_with(code));
    }

    #[test]
    fn scalar_and_structured_values_keep_existing_text_semantics() {
        for (value, text) in [
            (serde_json::json!("text"), "text"),
            (serde_json::json!(false), "false"),
            (serde_json::json!(42), "42"),
            (serde_json::Value::Null, "null"),
            (serde_json::json!([1, "two"]), "[1,\"two\"]"),
        ] {
            let script = shell_wrapper(&HashMap::from([("value".into(), value)]), "code").unwrap();
            assert!(script.contains(&format!("PARAM_VALUE='{text}'\nvalue='{text}'\n")));
        }
    }
}
