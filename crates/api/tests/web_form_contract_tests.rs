//! No database is needed: exercise the same JSON extractor used by API routes
//! with fixtures also asserted against the web forms' serialized requests.
use attune_api::{
    dto::{
        rule::UpdateRuleRequest,
        runtime::{
            CreateRuntimeRequest, NullableJsonPatch, NullableStringPatch, UpdateRuntimeRequest,
        },
        trigger::UpdateTriggerRequest,
    },
    openapi::ApiDoc,
};
use axum::{
    body::Body,
    extract::{rejection::JsonRejection, FromRequest},
    http::{Request, StatusCode},
    Json,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use utoipa::OpenApi;
use validator::Validate;

fn fixtures() -> Value {
    serde_json::from_str(include_str!(
        "../../../web/src/test/fixtures/form-requests.json"
    ))
    .unwrap()
}

#[test]
fn permission_grant_options_match_backend_resource_actions() {
    use attune_common::rbac::Resource;

    let fixture: Value = serde_json::from_str(include_str!(
        "../../../web/src/test/fixtures/permission-grant-options.json"
    ))
    .unwrap();
    let mut expected = serde_json::Map::new();
    for resource in Resource::ALL {
        let name = serde_json::to_value(resource).unwrap();
        let mut actions: Vec<String> =
            serde_json::from_value(serde_json::to_value(resource.supported_actions()).unwrap())
                .unwrap();
        actions.sort();
        assert!(expected
            .insert(name.as_str().unwrap().to_string(), json!(actions))
            .is_none());
    }
    let openapi = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let mut schema_resources: Vec<String> =
        serde_json::from_value(openapi["components"]["schemas"]["RbacResource"]["enum"].clone())
            .unwrap();
    schema_resources.sort();
    let mut backend_resources: Vec<_> = expected.keys().cloned().collect();
    backend_resources.sort();
    assert_eq!(backend_resources, schema_resources);
    assert_eq!(
        Value::Object(expected),
        fixture,
        "Permission editor contract drifted from backend grant validation"
    );
}

async fn extract<T: DeserializeOwned>(body: Value) -> Result<T, JsonRejection> {
    let request = Request::builder()
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    Json::<T>::from_request(request, &())
        .await
        .map(|Json(value)| value)
}

#[tokio::test]
async fn serialized_web_form_requests_deserialize_and_validate() {
    let fixtures = fixtures();
    extract::<UpdateRuleRequest>(fixtures["ruleUpdate"].clone())
        .await
        .unwrap()
        .validate()
        .unwrap();
    extract::<UpdateTriggerRequest>(fixtures["triggerUpdate"].clone())
        .await
        .unwrap()
        .validate()
        .unwrap();
    let clear = extract::<UpdateRuntimeRequest>(fixtures["runtimeClear"].clone())
        .await
        .unwrap();
    clear.validate().unwrap();
    assert!(matches!(
        clear.description,
        Some(NullableStringPatch::Clear)
    ));
    assert!(matches!(clear.installation, Some(NullableJsonPatch::Clear)));
    let set = extract::<UpdateRuntimeRequest>(fixtures["runtimeSet"].clone())
        .await
        .unwrap();
    set.validate().unwrap();
    assert!(matches!(set.description, Some(NullableStringPatch::Set(_))));
    assert!(matches!(set.installation, Some(NullableJsonPatch::Set(_))));
}

#[tokio::test]
async fn update_extractors_reject_identity_fields_and_malformed_patches() {
    for field in ["ref", "pack_ref"] {
        let body = json!({(field): "demo"});
        let rule = extract::<UpdateRuleRequest>(body.clone())
            .await
            .unwrap_err();
        let trigger = extract::<UpdateTriggerRequest>(body.clone())
            .await
            .unwrap_err();
        let runtime = extract::<UpdateRuntimeRequest>(body).await.unwrap_err();
        for error in [rule, trigger, runtime] {
            assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(error
                .body_text()
                .contains(&format!("unknown field `{field}`")));
        }
    }
    let error = extract::<UpdateRuntimeRequest>(json!({"description":{"op":"set"}}))
        .await
        .unwrap_err();
    assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error = extract::<UpdateRuleRequest>(json!({"permission_set_refs":[123]}))
        .await
        .unwrap_err();
    assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn partial_updates_are_accepted_without_optional_json_fields() {
    extract::<UpdateRuleRequest>(json!({"label":"Updated"}))
        .await
        .unwrap()
        .validate()
        .unwrap();
    extract::<UpdateRuntimeRequest>(json!({"name":"Updated"}))
        .await
        .unwrap()
        .validate()
        .unwrap();
}

#[tokio::test]
async fn runtime_installation_accepts_json_scalars() {
    let request = extract::<CreateRuntimeRequest>(json!({
        "ref": "demo.scalar",
        "name": "Scalar installation",
        "installation": "system",
        "execution_config": {},
        "distributions": {}
    }))
    .await
    .unwrap();
    request.validate().unwrap();
    assert_eq!(request.installation, Some(json!("system")));
}

#[test]
fn openapi_update_contracts_match_deserialization() {
    let spec = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let schemas = &spec["components"]["schemas"];
    for name in [
        "UpdateRuleRequest",
        "UpdateRuntimeRequest",
        "UpdateTriggerRequest",
    ] {
        let schema = &schemas[name];
        assert!(
            schema
                .get("required")
                .is_none_or(|required| required.as_array().unwrap().is_empty()),
            "{name} must allow partial updates"
        );
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"].get("pack_ref").is_none());
        assert!(schema["properties"].get("ref").is_none());
    }
    assert_eq!(
        schemas["UpdateRuleRequest"]["properties"]["permission_set_refs"]["items"]["type"],
        "string"
    );
    assert!(
        schemas["CreateRuntimeRequest"]["properties"]["installation"]
            .to_string()
            .contains("#/components/schemas/Value")
    );
}
