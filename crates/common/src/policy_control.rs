use crate::models::PolicyMethod;
use serde_yaml_ng::Value as YamlValue;

#[derive(Debug, PartialEq)]
pub(crate) struct PolicyControls {
    pub threshold: Option<i32>,
    pub method: Option<PolicyMethod>,
    pub parameters: Vec<String>,
    pub rate_limit_max_executions: Option<i32>,
    pub rate_limit_window_seconds: Option<i32>,
    pub quotas: serde_json::Value,
}

pub(crate) fn parse_policy_controls(data: &YamlValue) -> Result<PolicyControls, String> {
    let concurrency = data.get("concurrency");
    if concurrency.is_some_and(|value| !value.is_mapping()) {
        return Err("Policy concurrency must be an object".to_string());
    }
    let threshold = optional_i32(
        concurrency.and_then(|value| value.get("limit")),
        "concurrency.limit",
    )?;
    let method = concurrency
        .and_then(|value| value.get("method"))
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| "Policy concurrency.method must be cancel or enqueue".to_string())?;
            match value {
                "cancel" => Ok(PolicyMethod::Cancel),
                "enqueue" => Ok(PolicyMethod::Enqueue),
                other => Err(format!(
                    "Invalid policy method '{other}'; expected cancel or enqueue"
                )),
            }
        })
        .transpose()?;
    if threshold.is_some() != method.is_some() {
        return Err("Policy concurrency must include both limit and method".to_string());
    }
    let parameters = concurrency
        .and_then(|value| value.get("parameters"))
        .and_then(YamlValue::as_sequence)
        .map(|items| {
            items
                .iter()
                .filter_map(YamlValue::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let rate_limit = data.get("rate_limit");
    if rate_limit.is_some_and(|value| !value.is_mapping()) {
        return Err("Policy rate_limit must be an object".to_string());
    }
    let rate_limit_max_executions = optional_i32(
        rate_limit.and_then(|value| value.get("max_executions")),
        "rate_limit.max_executions",
    )?;
    let rate_limit_window_seconds = optional_i32(
        rate_limit.and_then(|value| value.get("window_seconds")),
        "rate_limit.window_seconds",
    )?;
    if rate_limit_max_executions.is_some() != rate_limit_window_seconds.is_some() {
        return Err(
            "Policy rate_limit must include both max_executions and window_seconds".to_string(),
        );
    }

    let quotas = parse_quotas(data.get("quotas"))?;
    if threshold.is_none()
        && rate_limit_max_executions.is_none()
        && quotas.as_array().is_none_or(Vec::is_empty)
    {
        return Err("Policy must configure concurrency, rate_limit, or quotas".to_string());
    }

    Ok(PolicyControls {
        threshold,
        method,
        parameters,
        rate_limit_max_executions,
        rate_limit_window_seconds,
        quotas,
    })
}

fn optional_i32(value: Option<&YamlValue>, field: &str) -> Result<Option<i32>, String> {
    value
        .map(|value| {
            let number = value
                .as_i64()
                .ok_or_else(|| format!("Policy {field} must be an integer"))?;
            let number = i32::try_from(number)
                .map_err(|_| format!("Policy {field} is outside the i32 range"))?;
            if number <= 0 {
                return Err(format!("Policy {field} must be greater than zero"));
            }
            Ok(number)
        })
        .transpose()
}

fn parse_quotas(value: Option<&YamlValue>) -> Result<serde_json::Value, String> {
    let Some(value) = value else {
        return Ok(serde_json::Value::Array(Vec::new()));
    };
    let items = value
        .as_sequence()
        .ok_or_else(|| "Policy quotas must be an array".to_string())?;
    let mut quotas = Vec::with_capacity(items.len());
    for item in items {
        let quota_type = item
            .as_mapping()
            .and_then(|_| item.get("quota_type"))
            .and_then(YamlValue::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Policy quota entries require quota_type".to_string())?;
        let limit = item
            .get("limit")
            .and_then(YamlValue::as_u64)
            .filter(|limit| *limit > 0)
            .ok_or_else(|| "Policy quota entries require positive limit".to_string())?;
        quotas.push(serde_json::json!({
            "quota_type": quota_type,
            "limit": limit,
        }));
    }
    Ok(serde_json::Value::Array(quotas))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<PolicyControls, String> {
        let value = serde_yaml_ng::from_str(yaml).unwrap();
        parse_policy_controls(&value)
    }

    #[test]
    fn parses_each_supported_control() {
        let controls = parse(
            "concurrency: { limit: 2, method: enqueue, parameters: [host] }\nrate_limit: { max_executions: 5, window_seconds: 60 }\nquotas: [{ quota_type: daily, limit: 10 }]\n",
        )
        .unwrap();
        assert_eq!(controls.threshold, Some(2));
        assert_eq!(controls.method, Some(PolicyMethod::Enqueue));
        assert_eq!(controls.parameters, ["host"]);
        assert_eq!(controls.rate_limit_max_executions, Some(5));
        assert_eq!(controls.rate_limit_window_seconds, Some(60));
        assert_eq!(controls.quotas[0]["limit"], 10);
    }

    #[test]
    fn rejects_incomplete_or_invalid_controls() {
        assert!(parse("concurrency: { limit: 2 }\n").is_err());
        assert!(parse("concurrency: { limit: 2, method: wait }\n").is_err());
        assert!(parse("rate_limit: { max_executions: 2 }\n").is_err());
        assert!(parse("concurrency: { limit: 0, method: cancel }\n").is_err());
        assert!(parse("rate_limit: { max_executions: 2, window_seconds: -1 }\n").is_err());
        assert!(parse("quotas: [{ quota_type: daily, limit: 0 }]\n").is_err());
        assert!(parse("enabled: true\n").is_err());
    }
}
