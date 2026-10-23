//! Fixtures owned only by the fresh ordinary-upload acceptance target.

use super::helpers::{Result, TestContext, TestResponse};
use attune_api::authz::AuthorizationService;
use attune_common::repositories::{
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
        PermissionAssignmentRepository, PermissionSetRepository, UpdatePermissionSetInput,
    },
    Create, Update,
};
use axum::{body::Body, http::Request};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};
use tower::ServiceExt;

pub struct Actor {
    pub id: i64,
    pub token: String,
}

/// IAM setup is deliberately separate from component deployment. These are the
/// actor's entire grants, not a role layered over an administrator fixture.
pub async fn actor(ctx: &TestContext, grants: Value) -> Result<Actor> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let login = format!("fresh_{suffix}");
    let response = ctx
        .post(
            "/auth/register",
            json!({"login":login,"password":"TestPassword123!"}),
            None,
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert!(status.is_success(), "{status}: {body}");
    let token = body["data"]["access_token"].as_str().unwrap().to_owned();
    let identity = IdentityRepository::find_by_login(&ctx.pool, &login)
        .await?
        .unwrap();
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("fixture.actor_{suffix}"),
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
            identity: identity.id,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    Ok(Actor {
        id: identity.id,
        token,
    })
}

pub async fn installer(ctx: &TestContext, refs: &[&str]) -> Result<Actor> {
    actor(ctx, json!([{"resource":"packs","actions":["read","install","configure"],"constraints":{"refs":refs}}])).await
}

/// Supported registrar mode requires explicit initial consent to each tick.
/// Keep the Packs-only installer unchanged for denial and deferred SOW cases.
pub async fn consenting_installer(ctx: &TestContext, refs: &[&str]) -> Result<Actor> {
    let ticks = refs
        .iter()
        .map(|reference| format!("{reference}.tick"))
        .collect::<Vec<_>>();
    actor(
        ctx,
        json!([
            {"resource":"packs","actions":["read","install","configure"],"constraints":{"refs":refs}},
            {"resource":"events","actions":["create"],"constraints":{"refs":ticks}}
        ]),
    )
    .await
}

/// Change only this actor's fixture-owned IAM set, never deployed pack policy.
pub async fn remove_tick_consent(
    ctx: &TestContext,
    installer: &Actor,
    refs: &[&str],
) -> Result<()> {
    let sets = PermissionSetRepository::find_by_identity(&ctx.pool, installer.id).await?;
    assert_eq!(sets.len(), 1);
    assert!(sets[0].r#ref.starts_with("fixture.actor_"));
    let grants = json!([{"resource":"packs","actions":["read","install","configure"],"constraints":{"refs":refs}}]);
    let updated = PermissionSetRepository::update(
        &ctx.pool,
        sets[0].id,
        UpdatePermissionSetInput {
            grants: Some(grants.clone()),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(updated.grants, grants);
    AuthorizationService::invalidate_identity_authz_cache(installer.id).await;
    Ok(())
}

pub struct Archive {
    pub root: tempfile::TempDir,
    pub reference: String,
    pub reverse: bool,
}

impl Archive {
    pub fn new(reference: &str, reverse: bool) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let fixture = Self {
            root,
            reference: reference.into(),
            reverse,
        };
        let p = reference;
        let mut files = BTreeMap::from([
            ("pack.yaml".to_owned(), format!("ref: {p}\nlabel: Fresh deployment acceptance\nversion: 1.0.0\nenabled: true\nconf_schema: {{}}\nconfig: {{}}\n")),
            ("runtimes/python.yaml".into(), format!("ref: {p}.python\nname: {p}_python\nexecution_config:\n  interpreter:\n    binary: python3\n    args: []\n    file_extension: .py\n")),
            ("triggers/tick.yaml".into(), format!("ref: {p}.tick\nlabel: Tick\noutput:\n  message: {{type: string, required: true}}\n")),
            ("permission_sets/events.yaml".into(), format!("ref: {p}.events\nscope: pack\ngrants:\n  - resource: events\n    actions: [create]\n    constraints:\n      refs: [{p}.tick]\n")),
            // The object-form expression preserves the original start-only match
            // in supported and deferred scenarios without changing their consent.
            ("rules/feedback.yaml".into(), format!("ref: {p}.feedback\nlabel: Feedback\ntrigger_ref: {p}.tick\naction_ref: {p}.outer\nconditions:\n  expression: \"{{{{ event.payload.message == 'start' }}}}\"\naction_params:\n  message: done\npermission_set_refs: [{p}.events]\n")),
            ("queues/inbox.yaml".into(), format!("ref: {p}.inbox\nlabel: Inbox\ndispatch_action: {p}.outer\npermission_set_refs: [{p}.events]\nbatch_mode: single\nitem_schema:\n  message: {{type: string, required: true}}\naction_params:\n  message: '{{{{ item.message }}}}'\n")),
            ("actions/workflows/inner.workflow.yaml".into(), format!("version: '1.0'\ntasks:\n  - name: emit\n    action: {p}.emit\n    input:\n      message: '{{{{ parameters.message }}}}'\n")),
            ("actions/workflows/outer.workflow.yaml".into(), format!("version: '1.0'\ntasks:\n  - name: nested\n    action: {p}.inner\n    input:\n      message: '{{{{ parameters.message }}}}'\n    next:\n      - when: '{{{{ succeeded() }}}}'\n        do: [finite]\n  - name: finite\n    action: '{{{{ parameters.target }}}}'\n    allowed_action_refs: [{p}.emit, {p}.alternate]\n    input:\n      message: '{{{{ parameters.message }}}}'\n")),
            ("actions/emit.py".into(), EMITTER.into()),
        ]);
        for (index, name) in ["outer", "inner", "emit", "alternate"].iter().enumerate() {
            // Reverse both lexical dependency order and tar entry order. Ref
            // identity does not change with the metadata filename.
            let ordinal = if reverse { 3 - index } else { index };
            let metadata = if ["outer", "inner"].contains(name) {
                format!("ref: {p}.{name}\nlabel: {name}\nworkflow_file: workflows/{name}.workflow.yaml\ndefault_execution_permission_set_refs: [{p}.events]\nparameters:\n  message: {{type: string, default: done}}\n  target: {{type: string, default: {p}.emit}}\n")
            } else {
                format!("ref: {p}.{name}\nlabel: {name}\nrunner_type: {p}.python\nentry_point: emit.py\nparameter_delivery: stdin\nparameter_format: json\noutput_format: json\ndefault_execution_permission_set_refs: [{p}.events]\nparameters:\n  message: {{type: string, default: done}}\noutput:\n  emitted: {{type: boolean}}\n")
            };
            files.insert(format!("actions/{ordinal}_{name}.yaml"), metadata);
        }
        let mut entries = files.into_iter().collect::<Vec<_>>();
        if reverse {
            entries.reverse();
        }
        for (path, body) in entries {
            fixture.write(&path, &body)?;
        }
        Ok(fixture)
    }

    pub fn write(&self, path: &str, contents: &str) -> Result<()> {
        let path = self.root.path().join(path);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, contents)?;
        Ok(())
    }

    pub fn replace(&self, path: &str, old: &str, new: &str) -> Result<()> {
        let contents = fs::read_to_string(self.root.path().join(path))?;
        assert!(contents.contains(old), "replacement must change {path}");
        self.write(path, &contents.replace(old, new))
    }

    pub async fn upload(&self, ctx: &TestContext, token: &str) -> Result<TestResponse> {
        self.upload_with_force(ctx, token, false).await
    }

    pub async fn upload_with_force(
        &self,
        ctx: &TestContext,
        token: &str,
        force: bool,
    ) -> Result<TestResponse> {
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(gzip);
        let mut paths = Vec::new();
        collect_files(self.root.path(), self.root.path(), &mut paths)?;
        paths.sort();
        if self.reverse {
            paths.reverse();
        }
        for path in paths {
            archive.append_path_with_name(self.root.path().join(&path), &path)?;
        }
        let archive = archive.into_inner()?.finish()?;
        let boundary = format!("fresh-{}", uuid::Uuid::new_v4().simple());
        // Fresh acceptance never forces replacement. The fixed-approval case
        // replaces only its explicitly created empty approval subject row.
        // No republish override, component IDs or alternate admission path.
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"force\"\r\n\r\n{force}\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"skip_tests\"\r\n\r\ntrue\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"pack\"; filename=\"pack.tar.gz\"\r\n\
             Content-Type: application/gzip\r\n\r\n"
        ).into_bytes();
        body.extend_from_slice(&archive);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/packs/upload")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body))?;
        Ok(TestResponse::new(ctx.app.clone().oneshot(request).await?))
    }
}

fn collect_files(root: &Path, directory: &Path, paths: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(root, &path, paths)?;
        } else {
            paths.push(path.strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}

// Standard library only, so the archive contains the actual runnable entrypoint
// without downloading an SDK. Parameters are stdin JSON, never environment data.
const EMITTER: &str = r#"import json
import os
import sys
import urllib.request

params = json.load(sys.stdin)
body = json.dumps({"trigger_ref": os.environ["ATTUNE_PACK_REF"] + ".tick",
                   "payload": {"message": params["message"]}}).encode()
request = urllib.request.Request(
    os.environ["ATTUNE_API_URL"].rstrip("/") + "/api/v1/events", data=body,
    headers={"Content-Type": "application/json",
             "Authorization": "Bearer " + os.environ["ATTUNE_API_TOKEN"]})
with urllib.request.urlopen(request, timeout=10) as response:
    if response.status != 201:
        raise RuntimeError("event creation failed")
print(json.dumps({"emitted": True}))
"#;
