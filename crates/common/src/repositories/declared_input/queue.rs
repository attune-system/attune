//! Queue-authored declarations come from retained dispatch metadata, never the
//! current queue projection or a contributor's payload.
use super::*;
use crate::repositories::runtime_authority::RuntimePolicyRevision;
use crate::runtime_authority::{InvocationBinding, InvocationPolicy};

pub(super) async fn append(
    connection: &mut PgConnection,
    policy: &RuntimePolicyRevision,
    invocation: &InvocationPolicy,
    target: Id,
    declarations: &mut Vec<DeclaredInputBinding>,
) -> Result<()> {
    if !matches!(invocation.binding, InvocationBinding::Queue { .. }) {
        return Ok(());
    }
    let source = invocation.source_id.ok_or_else(denied)?;
    let row: Option<(Id, Value, String)> = sqlx::query_as(
        "SELECT d.source_release,d.metadata,d.metadata_digest FROM managed_dispatch_metadata_revision d \
         JOIN pack_release r ON r.id=d.source_release AND r.pack=$3 \
         WHERE d.policy_revision=$1 AND d.component_kind='queues' AND d.component_id=$2 FOR SHARE OF d,r",
    ).bind(policy.id).bind(source).bind(policy.policy.owner.pack_id)
        .fetch_optional(connection).await?;
    let (release, metadata, expected) = row.ok_or_else(denied)?;
    if invocation.dispatch_template_digest.as_deref() != Some(expected.as_str())
        || hash(&metadata)? != expected
        || metadata["dispatch_action"].as_i64() != Some(target)
        || metadata["dispatch_action_ref"].as_str() != Some(invocation.binding.action_ref())
    {
        return Err(denied());
    }
    declarations.extend(from_metadata(
        target,
        policy.policy.owner.pack_id,
        release,
        &metadata,
    )?);
    if declarations.len() > 4096 {
        return Err(Error::validation("Too many authored input declarations"));
    }
    Ok(())
}

fn from_metadata(
    action: Id,
    pack: Id,
    release: Id,
    metadata: &Value,
) -> Result<Vec<DeclaredInputBinding>> {
    let params = metadata
        .get("action_params")
        .filter(|value| value.is_object())
        .ok_or_else(denied)?;
    let revision = hash(metadata)?;
    let mut leaves = Vec::new();
    collect(params, "/queue/action_params", &mut leaves)?;
    Ok(leaves
        .into_iter()
        .map(|(source_path, expression)| DeclaredInputBinding {
            action,
            pack,
            release,
            metadata_revision: None,
            declaration_revision: revision.clone(),
            source_path,
            expression,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queue_declaration_owns_exact_authored_paths_and_business_context() {
        let metadata = json!({"action_params":{
            "token":"{{ keystore.business_token }}",
            "endpoint":"{{ config.endpoint }}",
            "record":"{{ item.payload }}",
            "nested/path":{"~token":"{{ keystore.other }}"}
        },"dispatch_action":11,"dispatch_action_ref":"provider.run"});
        let sites = from_metadata(11, 22, 33, &metadata).unwrap();
        assert_eq!(sites.len(), 4);
        let token = sites
            .iter()
            .find(|site| site.source_path == "/queue/action_params/token")
            .unwrap();
        assert_eq!((token.action, token.pack, token.release), (11, 22, 33));
        assert!(token.uses_keystore());
        assert!(!token.uses_pack_configuration());
        assert!(sites
            .iter()
            .any(|site| site.source_path == "/queue/action_params/nested~1path/~0token"));
        let endpoint = sites
            .iter()
            .find(|site| site.source_path.ends_with("/endpoint"))
            .unwrap();
        assert!(endpoint.uses_pack_configuration());
        assert!(!endpoint.uses_keystore());
        // A selector change has different admission identity even at the same path.
        let mut changed = metadata.clone();
        changed["action_params"]["token"] = json!("{{ keystore.other }}");
        let changed = from_metadata(11, 22, 33, &changed).unwrap();
        assert!(changed
            .iter()
            .all(|site| site.declaration_revision != token.declaration_revision));
        assert!(!changed.contains(token));
        // Submitted data is not part of authored dispatch metadata.
        assert!(from_metadata(
            11,
            22,
            33,
            &json!({"payload":{"token":"{{ keystore.business_token }}"}})
        )
        .is_err());
    }
}
