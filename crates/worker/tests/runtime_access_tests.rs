use attune_worker::runtime::parameter_passing::{
    apply_runtime_environment, merge_parameters_and_secrets, prepare_parameters,
    ATTUNE_API_TOKEN_ENV,
};
use attune_worker::runtime::{ParameterDelivery, ParameterDeliveryConfig, ParameterFormat};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::process::Command;

#[test]
fn runtime_environment_fails_closed_without_an_explicit_execution_token() {
    let mut command = Command::new("echo");
    apply_runtime_environment(&mut command, &HashMap::new());

    let token = command
        .as_std()
        .get_envs()
        .find(|(key, _)| *key == ATTUNE_API_TOKEN_ENV)
        .map(|(_, value)| value);
    assert_eq!(token, Some(None));
}

#[test]
fn cache_values_and_api_credentials_are_not_action_input() {
    let parameters = HashMap::from([("requested_id".to_string(), json!("account-42"))]);
    let secrets = HashMap::from([("upstream_token".to_string(), json!("secret-value"))]);
    let merged = merge_parameters_and_secrets(&parameters, &secrets);
    let mut env = HashMap::from([
        (
            ATTUNE_API_TOKEN_ENV.to_string(),
            "execution-token".to_string(),
        ),
        (
            "CACHE_HTTP_RESPONSE".to_string(),
            r#"{"external_id":"account-42","value":"cache-data"}"#.to_string(),
        ),
    ]);

    let prepared = prepare_parameters(
        &merged,
        &mut env,
        ParameterDeliveryConfig {
            delivery: ParameterDelivery::Stdin,
            format: ParameterFormat::Json,
        },
    )
    .unwrap();
    let stdin: Value =
        serde_json::from_str(prepared.stdin_content().expect("stdin delivery")).unwrap();

    assert_eq!(stdin["requested_id"], "account-42");
    assert_eq!(stdin["upstream_token"], "secret-value");
    assert!(stdin.get(ATTUNE_API_TOKEN_ENV).is_none());
    assert!(stdin.get("CACHE_HTTP_RESPONSE").is_none());
    assert!(!prepared
        .stdin_content()
        .expect("stdin delivery")
        .contains("cache-data"));
}
