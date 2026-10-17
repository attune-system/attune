//! Current Access authority at dequeue, after actual socket readiness.
use super::*;

pub(super) async fn allowed(
    pool: &PgPool,
    auth: &WebSocketAuthContext,
    message: &ClientMessage,
) -> bool {
    async {
        let mut tx = pool.begin().await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
            .await?;
        let identity = IdentityRepository::find_by_id_for_share(&mut tx, auth.identity_id)
            .await?
            .filter(|identity| !identity.frozen)
            .ok_or_else(|| anyhow::anyhow!("Access identity unavailable"))?;
        let mut sets =
            PermissionSetRepository::find_by_identity_for_share(&mut tx, auth.identity_id).await?;
        sets.extend(
            PermissionSetRepository::find_by_identity_roles_for_share(&mut tx, auth.identity_id)
                .await?,
        );
        let mut seen = std::collections::HashSet::new();
        sets.retain(|set| seen.insert(set.id));
        let mut current = auth.clone();
        current.grants = attune_common::delegation::grants_from_sets(sets)?;
        current.roles = IdentityRoleAssignmentRepository::find_role_names_by_identity(
            &mut *tx,
            auth.identity_id,
        )
        .await?;
        current.identity_attributes = match identity.attributes {
            serde_json::Value::Object(attributes) => attributes.into_iter().collect(),
            _ => HashMap::new(),
        };
        if let ClientMessage::Notification(notification) = message {
            // These are metadata target locks, not native result disclosure.
            // Keep current identity/assignment/set locks through the target wait.
            use attune_common::repositories::runtime_target::{
                RuntimeTarget, RuntimeTargetRepository,
            };
            let target = match notification.entity_type.as_str() {
                "execution" => Some(RuntimeTarget::Execution(notification.entity_id)),
                "artifact" => Some(RuntimeTarget::Artifact(notification.entity_id)),
                _ => None,
            };
            if let Some(target) = target {
                RuntimeTargetRepository::resolve(&mut tx, target, auth.identity_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Notification target unavailable"))?;
            }
            if !notification_allowed_for_identity(notification, &current, pool).await {
                return Err(anyhow::anyhow!("Current notification grant unavailable"));
            }
        }
        tx.commit().await?;
        // No application await follows this check before start_send. IAM was
        // held through commit; a queued frame cannot reuse its handshake snapshot.
        if is_token_expired(auth.token_exp, chrono::Utc::now().timestamp()) {
            return Err(anyhow::anyhow!("Access token expired"));
        }
        Ok::<_, anyhow::Error>(())
    }
    .await
    .is_ok()
}
