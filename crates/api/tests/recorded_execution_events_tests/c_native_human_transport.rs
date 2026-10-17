//! Current Access IAM/JWT changes at the real native-source transport waits.
use super::c_native_transport::{Boundary, Checkpoint};
use super::*;
use attune_api::authz::AuthorizationService;
use attune_common::{
    auth::jwt::{generate_access_token, validate_token},
    rbac::{Action, Resource},
    repositories::identity::{
        CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
        PermissionAssignmentRepository, PermissionSetRepository,
    },
};

#[derive(Clone, Copy)]
pub(super) enum Change {
    Freeze,
    RevokeDecrypt,
    RevokeRead,
    Jwt,
    SourceDeadline,
}

pub(super) struct HumanReader {
    pub token: String,
    pub identity: Id,
    pub deadline: chrono::DateTime<chrono::Utc>,
    assignment: Id,
    control_token: String,
    change: Change,
}

async fn matrix(kind: usize) -> Result<()> {
    for change in [Change::Freeze, Change::RevokeDecrypt, Change::Jwt] {
        for boundary in [
            Boundary::Head,
            Boundary::Chunk,
            Boundary::Eof,
            Boundary::DownloadChunk,
            Boundary::DownloadEof,
        ] {
            // A prepared human HEAD holds the identity row FOR SHARE. A freeze
            // cannot commit inside that same target wait. Exercise freeze/IAM
            // after real blob waits, and JWT expiry at the locked HEAD seam.
            if matches!(boundary, Boundary::Head) && !matches!(change, Change::Jwt) {
                continue;
            }
            let at = Some(Checkpoint::Human(change, boundary));
            match kind {
                0 => super::c_native_execution_wait::execution_wait(at).await?,
                1 => {
                    super::c_native_related_wait::related_wait(
                        super::c_native_related_wait::Kind::Inquiry,
                        true,
                        at,
                    )
                    .await?
                }
                2 => {
                    super::c_native_related_wait::related_wait(
                        super::c_native_related_wait::Kind::Queue,
                        true,
                        at,
                    )
                    .await?
                }
                3 => super::c_native_cache::cache_http_case(false, 3600, at).await?,
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn human_execution_wait_access_freeze_source_decrypt_and_jwt_after_transport_waits(
) -> Result<()> {
    matrix(0).await
}
#[tokio::test]
async fn human_inquiry_wait_access_freeze_source_decrypt_and_jwt_after_transport_waits(
) -> Result<()> {
    matrix(1).await
}
#[tokio::test]
async fn human_queue_wait_access_freeze_source_decrypt_and_jwt_after_transport_waits() -> Result<()>
{
    matrix(2).await
}
#[tokio::test]
async fn human_cache_access_freeze_source_decrypt_and_jwt_after_transport_waits() -> Result<()> {
    matrix(3).await
}
#[tokio::test]
async fn human_cache_original_policy_deadline_after_head_chunk_and_eof_waits() -> Result<()> {
    for boundary in [
        Boundary::Head,
        Boundary::Chunk,
        Boundary::Eof,
        Boundary::DownloadChunk,
        Boundary::DownloadEof,
    ] {
        super::c_native_cache::cache_http_case(
            true,
            30,
            Some(Checkpoint::Human(Change::SourceDeadline, boundary)),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn create(
    fixture: &Fixture,
    template: Id,
    policy: Id,
    mut artifact_grants: Vec<Value>,
    change: Change,
    source_deadline: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<HumanReader> {
    use sha2::Digest;
    let mut tx = fixture.ctx.pool.begin().await?;
    let original = RuntimeAuthorityRepository::execution_admission(&mut tx, template)
        .await?
        .unwrap();
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, policy)
        .await?
        .unwrap();
    let mut grants = Vec::<attune_common::rbac::Grant>::new();
    for admitted in &original.authority.grants {
        // Same canonical digest as the repository's admitted-grant binding key.
        let key = sha2::Sha256::digest(serde_jcs::to_vec(admitted)?)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let binding = policy
            .policy
            .concrete_targets
            .get(&key)
            .ok_or("native reader binding unavailable")?;
        let targets = binding
            .targets
            .as_ref()
            .ok_or("native reader targets unavailable")?;
        assert!(!targets.is_empty());
        for target in targets {
            // IAM is allowed exact IDs. Do not turn an admitted narrow binding
            // back into a structural pack-wide human grant after source GC.
            let mut grant = admitted.grant().clone();
            grant.constraints = Some(serde_json::from_value(json!({"ids":[target.id]}))?);
            grants.push(grant);
        }
    }
    let credential = RuntimeCredentialBinding {
        execution_id: template,
        admission_id: original.id,
        sponsor_identity_id: original.authority.owner.sponsor_identity_id,
        action_ref: original.authority.binding.action_ref().to_owned(),
        attempt: original.authority.attempt,
    };
    let control_token = generate_admitted_execution_token(
        &credential,
        &original.authority.selected_refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    tx.rollback().await?;
    let login = format!("c_native_access_{}", uuid::Uuid::new_v4().simple());
    let identity = IdentityRepository::create(
        &fixture.ctx.pool,
        CreateIdentityInput {
            login: login.clone(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let mut decrypt = Vec::new();
    for grant in &mut grants {
        let action = if matches!(change, Change::RevokeRead) {
            Action::Read
        } else {
            Action::Decrypt
        };
        let selected = if matches!(change, Change::RevokeRead) {
            grant.resource == Resource::Executions
        } else {
            matches!(grant.resource, Resource::Keys | Resource::Caches)
        };
        if selected && grant.actions.contains(&action) {
            let mut only = grant.clone();
            only.actions = vec![action];
            decrypt.push(only);
            grant.actions.retain(|candidate| *candidate != action);
        }
    }
    assert!(
        !decrypt.is_empty(),
        "the current reader has a separately revocable exact grant"
    );
    artifact_grants.extend(
        grants
            .into_iter()
            .filter(|grant| !grant.actions.is_empty())
            .map(|grant| serde_json::to_value(grant).unwrap()),
    );
    for (suffix, body) in [
        ("base", serde_json::to_value(artifact_grants)?),
        ("decrypt", serde_json::to_value(decrypt)?),
    ] {
        let set = PermissionSetRepository::create(
            &fixture.ctx.pool,
            CreatePermissionSetInput {
                r#ref: format!("fixture.{login}_{suffix}"),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: body,
            },
        )
        .await?;
        let assignment = PermissionAssignmentRepository::create(
            &fixture.ctx.pool,
            CreatePermissionAssignmentInput {
                identity: identity.id,
                permset: set.id,
            },
        )
        .await?;
        if suffix == "decrypt" {
            AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
            AuthorizationService::invalidate_permission_set_caches().await;
            let mut config = fixture.ctx.state.jwt_config.as_ref().clone();
            if matches!(change, Change::Jwt) {
                config.access_token_expiration = 8;
            }
            let token = generate_access_token(identity.id, &identity.login, &config)?;
            let claims = validate_token(&token, &fixture.ctx.state.jwt_config)?;
            let deadline = if matches!(change, Change::SourceDeadline) {
                source_deadline.ok_or("original source deadline required")?
            } else if matches!(change, Change::Jwt) {
                chrono::DateTime::from_timestamp(claims.exp, 0).unwrap()
            } else {
                chrono::Utc::now() + chrono::Duration::seconds(30)
            };
            assert!(
                source_deadline.is_none() || matches!(change, Change::SourceDeadline),
                "current Access failures must not borrow an original cache deadline"
            );
            return Ok(HumanReader {
                token,
                identity: identity.id,
                deadline,
                assignment: assignment.id,
                control_token,
                change,
            });
        }
    }
    unreachable!()
}

impl HumanReader {
    pub(super) fn changes_identity_or_read(&self) -> bool {
        matches!(self.change, Change::Freeze | Change::RevokeRead)
    }
    pub(super) async fn change_after_wait(&self, fixture: &Fixture, execution: Id) -> Result<()> {
        match self.change {
            Change::Freeze => {
                IdentityRepository::update(
                    &fixture.ctx.pool,
                    self.identity,
                    UpdateIdentityInput {
                        frozen: Some(true),
                        ..Default::default()
                    },
                )
                .await?;
            }
            Change::RevokeDecrypt | Change::RevokeRead => {
                assert!(
                    PermissionAssignmentRepository::delete(&fixture.ctx.pool, self.assignment)
                        .await?
                );
                AuthorizationService::invalidate_identity_authz_cache(self.identity).await;
                AuthorizationService::invalidate_permission_set_caches().await;
                let sets =
                    PermissionSetRepository::find_by_identity(&fixture.ctx.pool, self.identity)
                        .await?;
                let mut read = false;
                for set in sets {
                    for grant in
                        serde_json::from_value::<Vec<attune_common::rbac::Grant>>(set.grants)?
                    {
                        if matches!(self.change, Change::RevokeRead)
                            && grant.resource == Resource::Executions
                        {
                            assert!(
                                !grant.actions.contains(&Action::Read),
                                "current Access execution Read must actually be revoked"
                            );
                        }
                        if matches!(grant.resource, Resource::Keys | Resource::Caches) {
                            read |= grant.actions.contains(&Action::Read);
                            if matches!(self.change, Change::RevokeDecrypt) {
                                assert!(
                                    !grant.actions.contains(&Action::Decrypt),
                                    "source Decrypt revocation must reach raw IAM"
                                );
                            }
                        }
                    }
                }
                assert!(read, "source Read survives independent Decrypt revocation");
            }
            Change::Jwt | Change::SourceDeadline => {
                super::c_native_transport::expires(self.deadline).await?
            }
        }
        if !matches!(self.change, Change::SourceDeadline) {
            assert_eq!(
                fixture
                    .ctx
                    .get(
                        &format!("/api/v1/executions/{execution}?include_secret_values=true"),
                        Some(&self.control_token)
                    )
                    .await?
                    .status(),
                StatusCode::OK,
                "the independent recorded reader and original source consents remain valid"
            );
        }
        Ok(())
    }
}
