//! Recorded execution credentials never expand through mutable identity IAM.

use attune_common::{
    auth::Claims,
    rbac::Action,
    repositories::{
        runtime_authority::{
            ApprovalEffectFence, RuntimeAuthorityRepository, RuntimeCredentialBinding,
        },
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
    },
};
use sqlx::PgPool;

use crate::service::Notification;

#[derive(Clone)]
pub(crate) struct RecordedConnection {
    pub(crate) binding: RuntimeCredentialBinding,
    selected_refs: Vec<String>,
}

impl RecordedConnection {
    pub(crate) fn selected_refs(&self) -> &[String] {
        &self.selected_refs
    }
    pub(crate) fn from_claims(claims: &Claims) -> anyhow::Result<Option<Self>> {
        claims
            .admitted_execution_credential()?
            .map(|binding| {
                Ok(Self {
                    binding,
                    selected_refs: claims.admitted_permission_set_refs()?,
                })
            })
            .transpose()
    }

    /// No live permit is cached. Database failure closes the connection.
    pub(crate) async fn is_live(&self, pool: &PgPool) -> bool {
        self.source_fence(pool).await.is_some()
    }

    pub(crate) async fn source_fence(&self, pool: &PgPool) -> Option<ApprovalEffectFence> {
        let mut tx = pool.begin().await.ok()?;
        let authority =
            RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &self.binding)
                .await
                .ok()?;
        if authority.authority().selected_refs != self.selected_refs {
            return None;
        }
        authority.ensure_fresh_on(&mut tx).await.ok()?;
        Some(authority.effect_fence().clone())
    }

    pub(crate) async fn permits(&self, pool: &PgPool, notification: &Notification) -> bool {
        self.notification_fence(pool, notification).await.is_some()
    }

    pub(crate) async fn notification_fence(
        &self,
        pool: &PgPool,
        notification: &Notification,
    ) -> Option<ApprovalEffectFence> {
        // Event identity requires its partition key. Unknown notification kinds
        // and ambiguous event IDs cannot establish a structural owner.
        let target = match notification.entity_type.as_str() {
            "event" => {
                let Some(created) = notification
                    .payload
                    .get("created")
                    .and_then(|value| value.as_str())
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                else {
                    return None;
                };
                RuntimeTarget::Event {
                    id: notification.entity_id,
                    created: created.with_timezone(&chrono::Utc),
                }
            }
            "execution" => RuntimeTarget::Execution(notification.entity_id),
            "enforcement" => RuntimeTarget::Enforcement(notification.entity_id),
            "artifact" => RuntimeTarget::Artifact(notification.entity_id),
            "work_queue" => {
                RuntimeTarget::Component(ComponentTarget::Queue, notification.entity_id)
            }
            "work_queue_item" => RuntimeTarget::QueueItem(notification.entity_id),
            "rule_lifecycle" => {
                RuntimeTarget::Component(ComponentTarget::Rule, notification.entity_id)
            }
            _ => return None,
        };
        let Ok(mut tx) = pool.begin().await else {
            return None;
        };
        let Ok(authority) =
            RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &self.binding).await
        else {
            return None;
        };
        if authority.authority().selected_refs != self.selected_refs {
            return None;
        }
        let mut expected_links = None;
        if notification.entity_type == "artifact" {
            let Ok(links) = attune_common::repositories::artifact::ArtifactRepository::linked_execution_ids_bounded(
                &mut *tx, notification.entity_id, 257,
            ).await else { return None; };
            if links.len() > 256 {
                return None;
            }
            for execution in links.iter().copied() {
                if execution == self.binding.execution_id {
                    continue;
                }
                let Ok(Some(target)) = RuntimeTargetRepository::resolve(
                    &mut tx,
                    RuntimeTarget::Execution(execution),
                    self.binding.sponsor_identity_id,
                )
                .await
                else {
                    return None;
                };
                if !authority.allows_target(&target, Action::Read) {
                    return None;
                }
            }
            expected_links = Some(links);
        }
        // Match worker completion's execution-before-artifact order. No
        // execution locks may be acquired after the artifact lock.
        let Ok(Some(target)) =
            RuntimeTargetRepository::resolve(&mut tx, target, self.binding.sponsor_identity_id)
                .await
        else {
            return None;
        };
        if !authority.allows_target(&target, Action::Read) {
            return None;
        }
        if let Some(expected) = expected_links {
            let Ok(current) = attune_common::repositories::artifact::ArtifactRepository::linked_execution_ids_bounded(
                &mut *tx, notification.entity_id, 257,
            ).await else { return None; };
            if current != expected {
                return None;
            }
        }
        authority.ensure_fresh_on(&mut tx).await.ok()?;
        Some(authority.effect_fence().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::{jwt::generate_admitted_execution_token, validate_token, JwtConfig};

    #[test]
    fn recorded_claims_cannot_downgrade_to_identity_iam() {
        attune_common::auth::install_crypto_provider();
        let config = JwtConfig {
            secret: "notifier-recorded-test".into(),
            access_token_expiration: 60,
            refresh_token_expiration: 60,
        };
        let binding = RuntimeCredentialBinding {
            execution_id: 11,
            admission_id: 12,
            sponsor_identity_id: 13,
            action_ref: "fixture.action".into(),
            attempt: uuid::Uuid::new_v4(),
        };
        let token =
            generate_admitted_execution_token(&binding, &["standard".into()], &config, Some(60))
                .unwrap();
        let claims = validate_token(&token, &config).unwrap();
        let recorded = RecordedConnection::from_claims(&claims).unwrap().unwrap();
        assert_eq!(recorded.binding.execution_id, 11);
        let mut malformed = claims.clone();
        malformed.metadata.as_mut().unwrap()["authority_source"] =
            serde_json::json!("human_delegation");
        assert!(RecordedConnection::from_claims(&malformed).is_err());
        malformed = claims.clone();
        malformed.metadata.as_mut().unwrap()["permission_set_refs"] =
            serde_json::json!(["standard", 7]);
        assert!(RecordedConnection::from_claims(&malformed).is_err());
        malformed = claims;
        malformed.sub = "999".into();
        assert!(RecordedConnection::from_claims(&malformed).is_err());
    }
}
