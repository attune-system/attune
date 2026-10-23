//! Public schema and real-route metadata boundaries. Database tests are executed
//! only by the central PG18 validation lane, never by a client generator.
mod helpers;

use attune_api::{
    authz::AuthorizationService, dto::runtime_approval::IssueDeclaredInputApprovalRequest,
};
use attune_common::{
    delegation::DelegationAuthority,
    models::OwnerType,
    pack_registry::{deployment_admission::DeploymentAdmission, PackComponentLoader},
    repositories::{
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        key::{CreateKeyInput, KeyRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        Create, FindByRef,
    },
};
use axum::{
    body::Body,
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use helpers::{Result, TestContext};
use serde_json::{json, Value};
use tower::ServiceExt;
use utoipa::OpenApi;

#[test]
fn request_and_openapi_expose_exclusive_typed_targets_and_authored_grants() {
    let base = json!({"declaration_revision":"revision","source_path":"/workflow/vars/token","expression":"{{ keystore['system.token'] }}","grants":[{"resource":"keys","actions":["read"],"scope":"external","constraints":{"refs":["system.token"]}}]});
    let mut renderer = base.clone();
    renderer["renderer_execution"] = json!(1);
    assert!(serde_json::from_value::<IssueDeclaredInputApprovalRequest>(renderer.clone()).is_ok());
    let mut policy = base;
    policy["policy_revision"] = json!(2);
    policy["invocation_binding"] = json!({"kind":"action","action_ref":"example.flow"});
    assert!(serde_json::from_value::<IssueDeclaredInputApprovalRequest>(policy.clone()).is_ok());
    renderer["policy_revision"] = json!(2);
    renderer["invocation_binding"] = policy["invocation_binding"].clone();
    assert!(serde_json::from_value::<IssueDeclaredInputApprovalRequest>(renderer).is_err());
    for field in [
        "purpose",
        "proof",
        "permission_set_refs",
        "issuer_identity_id",
    ] {
        let mut invalid = policy.clone();
        invalid[field] = json!("not a caller-controlled field");
        assert!(serde_json::from_value::<IssueDeclaredInputApprovalRequest>(invalid).is_err());
    }
    let api = serde_json::to_value(attune_api::openapi::ApiDoc::openapi()).unwrap();
    let schemas = &api["components"]["schemas"];
    assert_eq!(
        schemas["IssueDeclaredInputApprovalRequest"]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        schemas["InvocationBinding"]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let authored = &schemas["AuthoredGrantConstraints"]["properties"];
    assert!(authored.get("refs").is_some());
    assert!(authored.get("ids").is_none());
    assert!(authored.get("managing_pack_id").is_none());
    assert!(
        schemas["PolicyInputApprovalRequest"]["properties"]["invocation_binding"]
            .to_string()
            .contains("InvocationBinding")
    );
    assert!(
        schemas["PolicyInputApprovalRequest"]["properties"]["grants"]
            .to_string()
            .contains("PermissionGrantDefinition")
    );
    let discovery = &api["paths"]["/api/v1/packs/{pack_ref}/input-approval-targets"]["get"];
    assert!(discovery.get("security").is_some());
    assert!(
        api["paths"]["/api/v1/packs/{pack_ref}/external-input-approvals"]["post"]
            .get("requestBody")
            .is_some()
    );
}

#[test]
fn archive_response_exports_have_raw_binary_schemas_for_client_parsers() {
    let spec = serde_json::to_value(attune_api::openapi::ApiDoc::openapi()).unwrap();
    for (path, operation, statuses) in [
        (
            "/api/v1/internal/packs/{pack_ref}/archive",
            "download_pack_archive",
            &["200", "206"][..],
        ),
        (
            "/api/v1/internal/pack-installs/{pack_install_id}/archive",
            "download_pack_install_candidate_archive",
            &["200"][..],
        ),
    ] {
        let get = &spec["paths"][path]["get"];
        assert_eq!(get["operationId"], operation);
        assert!(get.get("security").is_some());
        for status in statuses {
            let content = &get["responses"][*status]["content"];
            let schema = content["application/gzip"]
                .get("schema")
                .expect("Archive responses must declare a body, not only a media type");
            assert_eq!(schema["type"], "string", "{path} {status}: {schema}");
            assert_eq!(schema["format"], "binary", "{path} {status}: {schema}");
            // This is the scalar binary/File property recognized by client
            // generators, not JSON Vec<u8>, an encoded string or an empty schema.
            assert!(schema.get("items").is_none());
            assert!(schema.get("$ref").is_none());
            assert!(schema.get("contentEncoding").is_none());
            assert!(content.get("application/json").is_none());
        }
    }
}

async fn grant(ctx: &TestContext, reference: &str, grants: Value) -> Result<()> {
    let identity = ctx.user.as_ref().ok_or("identity missing")?.id;
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: reference.into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

async fn deploy(ctx: &TestContext, expression: &str) -> Result<()> {
    let pack = helpers::create_test_pack(&ctx.pool, "public_inputs").await?;
    let source = ctx.test_packs_dir.join("public-input-declarations");
    std::fs::create_dir_all(source.join("actions/workflows"))?;
    std::fs::create_dir_all(source.join("runtimes"))?;
    std::fs::write(
        source.join("pack.yaml"),
        "ref: public_inputs\nversion: '1.0.0'\n",
    )?;
    std::fs::write(
        source.join("runtimes/native.yaml"),
        "ref: public_inputs.native\nname: native\nexecution_config: {}\n",
    )?;
    std::fs::write(source.join("actions/leaf.yaml"),format!("ref: public_inputs.leaf\nrunner_type: native\nentry_point: leaf.sh\nparameters:\n  token:\n    type: string\n    default: {}\n",serde_json::to_string(expression)?))?;
    std::fs::write(
        source.join("actions/leaf.sh"),
        "#!/bin/sh\nprintf '{}\\n'\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            source.join("actions/leaf.sh"),
            std::fs::Permissions::from_mode(0o755),
        )?;
    }
    std::fs::write(source.join("actions/flow.yaml"),"ref: public_inputs.flow\nreference_visibility: private\nworkflow_file: workflows/flow.workflow.yaml\n")?;
    std::fs::write(
        source.join("actions/workflows/flow.workflow.yaml"),
        format!(
            "version: '1.0'\nvars:\n  token: {}\ntasks:\n  send:\n    action: public_inputs.leaf\n",
            serde_json::to_string(expression)?
        ),
    )?;
    let admission = DeploymentAdmission::read(&source, &pack.r#ref)?;
    let mut tx = ctx.pool.begin().await?;
    DeploymentAdmission::lock(&mut tx).await?;
    DeploymentAdmission::preserve(
        &mut tx,
        pack.id,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let installer = DelegationAuthority::load_for_share(
        &mut tx,
        ctx.user.as_ref().ok_or("installer missing")?.id,
    )
    .await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest: "c".repeat(64),
            object_key: "packs/blobs/public-inputs.tar.gz".into(),
            provider_version: "e:public-inputs".into(),
            content_path: source.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await?;
    let loader = PackComponentLoader::new(
        &ctx.pool,
        pack.id,
        &pack.r#ref,
        &ctx.state.config.cache_admission,
        installer.clone(),
    );
    let loaded = admission.load(loader, &mut tx).await?;
    assert_eq!(loaded.total_skipped(), 0, "{:?}", loaded.warnings);
    PackReleaseRepository::activate_projected(&mut tx, pack.id, release.id, &loaded.projections)
        .await?;
    admission
        .finish(
            &mut tx,
            pack.id,
            release.id,
            ctx.state.config.security.encryption_key.as_deref(),
            &installer,
        )
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn preexecution_discovery_and_shared_ledger_preserve_private_source_visibility_and_exact_targets(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let key = KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "public_input_source".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Source value must never appear in metadata".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("SOURCE_VALUE_CANARY"),
        },
    )
    .await?;
    let expression = format!("{{{{ keystore['{}'] }}}}", key.r#ref);
    deploy(&ctx, &expression).await?;
    let execution_count: i64 = sqlx::query_scalar("SELECT count(*) FROM execution")
        .fetch_one(&ctx.pool)
        .await?;
    let endpoint = "/api/v1/packs/public_inputs/input-approval-targets";
    // TestContext::get(None) inherits the fixture's access token. Exercise the
    // actual unauthenticated boundary without changing shared helper semantics.
    let request = Request::get(endpoint).body(Body::empty())?;
    assert!(!request.headers().contains_key(AUTHORIZATION));
    let response = ctx.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        ctx.get(endpoint, ctx.token()).await?.status(),
        StatusCode::FORBIDDEN
    );
    grant(
        &ctx,
        "fixture.public_input_pack",
        json!([{"resource":"packs","actions":["read"],"constraints":{"refs":["public_inputs"]}}]),
    )
    .await?;
    let public: Value = ctx
        .get(&format!("{endpoint}?page_size=1"), ctx.token())
        .await?
        .json()
        .await?;
    assert_eq!(
        public["pagination"]["total_items"], 1,
        "Only the public leaf's own default is visible, not private workflow invocations: {public}"
    );
    assert_eq!(public["items"][0]["invocation_binding"]["kind"], "action");
    assert_eq!(public["items"][0]["action_ref"], "public_inputs.leaf");
    let page2: Value = ctx
        .get(&format!("{endpoint}?page_size=1&page=2"), ctx.token())
        .await?
        .json()
        .await?;
    assert!(page2["items"].as_array().ok_or("items missing")?.is_empty());
    assert_eq!(page2["pagination"]["total_items"], 1);
    grant(&ctx,"fixture.public_input_flow",json!([{"resource":"actions","actions":["read"],"constraints":{"refs":["public_inputs.flow"]}},{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[key.r#ref]}}])).await?;
    let visible: Value = ctx.get(endpoint, ctx.token()).await?.json().await?;
    assert_eq!(visible["pagination"]["total_items"], 3, "{visible}");
    let target = visible["items"]
        .as_array()
        .ok_or("targets missing")?
        .iter()
        .find(|target| target["action_ref"] == "public_inputs.flow")
        .ok_or("private flow target missing")?
        .clone();
    assert_eq!(target["source_path"], "/workflow/vars/token");
    assert_eq!(target["expression"], expression);
    assert!(visible["items"]
        .as_array()
        .ok_or("items missing")?
        .iter()
        .any(|target| target["invocation_binding"]["kind"] == "workflow_task"));
    let grants = json!([{"resource":"keys","actions":["read","decrypt"],"scope":"external","constraints":{"refs":[key.r#ref]}}]);
    let request = json!({"policy_revision":target["policy_revision"],"invocation_binding":target["invocation_binding"],"declaration_revision":target["declaration_revision"],"source_path":target["source_path"],"expression":target["expression"],"grants":grants});
    let response = ctx
        .post(
            "/api/v1/packs/public_inputs/external-input-approvals",
            request,
            ctx.token(),
        )
        .await?;
    let status = response.status();
    let issued: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    assert_eq!(
        issued["data"]["purpose"],
        json!({"kind":"declared_input","targets":[target.clone()]})
    );
    assert!(!issued.to_string().contains("SOURCE_VALUE_CANARY"));
    let id = issued["data"]["id"].as_i64().ok_or("approval id missing")?;
    let runtime: Value = ctx
        .post(
            "/api/v1/packs/public_inputs/external-approvals",
            json!({"grants":grants}),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(runtime["data"]["purpose"], json!({"kind":"runtime"}));
    let ledger: Value = ctx
        .get(
            "/api/v1/packs/public_inputs/external-approvals",
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(ledger["pagination"]["total_items"], 2);
    let revoked: Value = ctx
        .post(
            &format!("/api/v1/packs/public_inputs/external-approvals/{id}/revoke"),
            json!({}),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(revoked["data"]["active"], false);
    assert_eq!(revoked["data"]["purpose"], issued["data"]["purpose"]);
    let mut tx = ctx.pool.begin().await?;
    assert!(
        attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::revoke_policy(
            &mut tx,
            target["policy_revision"].as_i64().ok_or("policy missing")?,
        )
        .await?
    );
    tx.commit().await?;
    let historical: Value = ctx.get(endpoint, ctx.token()).await?.json().await?;
    assert_eq!(
        historical["pagination"]["total_items"], 3,
        "Metadata remains inspectable after policy revocation"
    );
    let denied = ctx.post("/api/v1/packs/public_inputs/external-input-approvals",json!({"policy_revision":target["policy_revision"],"invocation_binding":target["invocation_binding"],"declaration_revision":target["declaration_revision"],"source_path":target["source_path"],"expression":target["expression"],"grants":grants}),ctx.token()).await?;
    assert_eq!(
        denied.status(),
        StatusCode::FORBIDDEN,
        "Discovery metadata must not bypass the original revoked policy"
    );
    // Retiring and replacing a same-ref source cannot authorize old graph metadata.
    sqlx::query("UPDATE action SET retired_at=NOW(),ref=ref||'_retired' WHERE id=$1")
        .bind(target["action_id"].as_i64().ok_or("action missing")?)
        .execute(&ctx.pool)
        .await?;
    let pack =
        attune_common::repositories::pack::PackRepository::find_by_ref(&ctx.pool, "public_inputs")
            .await?
            .ok_or("pack missing")?;
    helpers::create_test_action(&ctx.pool, pack.id, &pack.r#ref, "public_inputs.flow").await?;
    let retained: Value = ctx
        .get(
            &format!("/api/v1/packs/public_inputs/external-approvals/{id}"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(
        retained["data"]["purpose"],
        json!({"kind":"declared_input","targets":[]})
    );
    let remaining: Value = ctx.get(endpoint, ctx.token()).await?.json().await?;
    assert_eq!(remaining["pagination"]["total_items"], 1);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM execution")
        .fetch_one(&ctx.pool)
        .await?;
    assert_eq!(
        after, execution_count,
        "Discovery and consent must not create an execution"
    );
    ctx.cleanup().await?;
    Ok(())
}
