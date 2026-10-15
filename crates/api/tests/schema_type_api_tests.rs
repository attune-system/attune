mod helpers;

use attune_common::{
    repositories::{component_lifecycle::PackProjectionIds, schema_type::SchemaTypeRepository},
    schema_type::SchemaTypeDefinition,
    value_schema::{compile_inline, SchemaContext},
};
use axum::http::StatusCode;
use helpers::{create_test_pack, TestContext};
use serde_json::json;

#[tokio::test]
async fn rule_authoring_resolves_named_secrets_under_component_pack_policy() {
    let context = TestContext::new().await.unwrap();
    let pack = create_test_pack(&context.pool, "schema-rule-owned")
        .await
        .unwrap();
    let definition = SchemaTypeDefinition::parse("ref: schema-rule-owned.token\nlabel: Token\nreference_visibility: private\nschema:\n  type: string\n  secret: true\n",&pack.r#ref).unwrap();
    let contract = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let mut tx = context.pool.begin().await.unwrap();
    let row = SchemaTypeRepository::upsert(&mut tx, pack.id, &definition, &contract)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    helpers::activate_test_pack_release_with_projections(
        &context.pool,
        &pack,
        &PackProjectionIds {
            schema_types: vec![row.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut action =
        helpers::create_test_action(&context.pool, pack.id, &pack.r#ref, "schema-rule-owned.run")
            .await
            .unwrap();
    action.param_schema = Some(json!({"token":{"type":definition.r#ref}}));
    let params = json!({"token":"{{ keystore['system.rule_input'] }}"});
    attune_api::validation::params::validate_action_params_with_registry(
        &context.pool,
        &action,
        &params,
    )
    .await
    .unwrap();
    let error = attune_api::validation::params::validate_action_params_with_registry(
        &context.pool,
        &action,
        &json!({"token":"literal-fixture"}),
    )
    .await
    .unwrap_err();
    assert!(!error.to_string().contains("literal-fixture"));
    let other = create_test_pack(&context.pool, "schema-rule-unrelated")
        .await
        .unwrap();
    action.pack = other.id;
    action.pack_ref = other.r#ref.clone();
    action.r#ref = format!("{}.run", other.r#ref);
    assert!(
        attune_api::validation::params::validate_action_params_with_registry(
            &context.pool,
            &action,
            &params
        )
        .await
        .is_err()
    );
    context.cleanup().await.unwrap();
}

#[tokio::test]
async fn schema_discovery_requires_authentication_but_no_schema_permissions() {
    let context = TestContext::new().await.unwrap();
    let pack = create_test_pack(&context.pool, "schema-api-private")
        .await
        .unwrap();
    let definition = SchemaTypeDefinition::parse("ref: schema-api-private.value\nlabel: Private value\nreference_visibility: private\nschema:\n  type: string\n", &pack.r#ref).unwrap();
    let compiled = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let mut transaction = context.pool.begin().await.unwrap();
    let row = SchemaTypeRepository::upsert(&mut transaction, pack.id, &definition, &compiled)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    helpers::activate_test_pack_release_with_projections(
        &context.pool,
        &pack,
        &PackProjectionIds {
            schema_types: vec![row.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        context
            .get("/api/v1/schema-types", None)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let login = context.post("/auth/register", json!({"login": "schema-reader@example.test", "password": "TestPassword123!", "display_name": "Schema reader"}), None).await.unwrap();
    assert!(login.status().is_success());
    let body: serde_json::Value = login.json().await.unwrap();
    let token = body["data"]["access_token"].as_str().unwrap();
    let response = context
        .get("/api/v1/schema-types?q=private", Some(token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["items"][0]["ref"], definition.r#ref);
    for path in [
        "/api/v1/schema-types/schema-api-private.value",
        "/api/v1/schema-types/schema-api-private.value/inspect",
        "/api/v1/packs/schema-api-private/schema-types",
    ] {
        assert_eq!(
            context.get(path, Some(token)).await.unwrap().status(),
            StatusCode::OK
        );
    }
    let response = context
        .post(
            "/api/v1/schema-types/preview",
            json!({"context": "single_value", "schema": {"type": definition.r#ref}}),
            Some(token),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["data"]["contract"]["dependencies"][0]["type_ref"],
        definition.r#ref
    );
    assert!(body["data"]["semantic_digest"].is_string());
    context.cleanup().await.unwrap();
}
