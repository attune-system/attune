//! Producer-side dispatch retention, before the captured policy is activated.

use super::ManagedComponentReadmission;
use crate::{
    models::Id,
    repositories::{
        managed_executable::ManagedExecutableRepository,
        runtime_authority::{RuntimeAuthorityRepository, RuntimePolicyRevision},
        schema_contract::SchemaContractRepository,
        schema_type::SchemaTypeRepository,
    },
    runtime_authority::InvocationBinding,
    value_schema::digest,
    Error, Result,
};
use serde_json::Value;
use sqlx::PgConnection;

impl ManagedComponentReadmission {
    /// Persist all real rule/queue source bodies against this exact captured
    /// policy in its creation transaction. This never readmits a component or
    /// activates another policy. Call before activation and sensor assessment.
    /// Retained sources copy their original evidence, never today's projection.
    pub async fn capture_deployment_dispatch(
        connection: &mut PgConnection,
        revision: &RuntimePolicyRevision,
    ) -> Result<()> {
        SchemaTypeRepository::lock_reclamation(connection).await?;
        let stored = RuntimeAuthorityRepository::find_policy(connection, revision.id)
            .await?
            .ok_or_else(|| Error::validation("Dispatch policy is not captured"))?;
        if stored.revoked_at.is_some()
            || stored.policy_digest != revision.policy_digest
            || digest(&serde_json::to_value(&revision.policy.0)?)
                .map_err(|_| Error::validation("Invalid dispatch policy digest"))?
                != stored.policy_digest
        {
            return Err(Error::validation(
                "Dispatch policy differs from its captured body",
            ));
        }
        let fresh: bool = sqlx::query_scalar(
            "SELECT xmin::text::numeric=mod(pg_current_xact_id()::text::numeric,4294967296) FROM pack_runtime_policy_revision WHERE id=$1 FOR SHARE",
        ).bind(stored.id).fetch_one(&mut *connection).await?;
        for invocation in &stored.policy.invocations {
            let (kind, reference, table, target_column, target_ref_column) =
                match &invocation.binding {
                    InvocationBinding::Rule { rule_ref, .. } => {
                        ("rules", rule_ref, "rule", "action", "action_ref")
                    }
                    InvocationBinding::Queue { queue_ref, .. } => (
                        "queues",
                        queue_ref,
                        "work_queue",
                        "dispatch_action",
                        "dispatch_action_ref",
                    ),
                    _ => continue,
                };
            let source = invocation
                .source_id
                .ok_or_else(|| Error::validation("Dispatch source identity is not captured"))?;
            let hash = invocation
                .dispatch_template_digest
                .as_deref()
                .ok_or_else(|| Error::validation("Dispatch source digest is not captured"))?;
            let present: Option<(Id, Value, String)> = sqlx::query_as(
                "SELECT source_release,metadata,metadata_digest FROM managed_dispatch_metadata_revision WHERE policy_revision=$1 AND component_kind=$2 AND component_id=$3 FOR SHARE",
            ).bind(stored.id).bind(kind).bind(source).fetch_optional(&mut *connection).await?;
            if present.is_none() && !fresh {
                return Err(Error::PermissionDenied(
                    "Historical policy cannot acquire deployment dispatch metadata".into(),
                ));
            }
            // The policy owns exact executable/schema pins. Do not discover a
            // publisher-current action by ref when retaining this source body.
            let target =
                ManagedExecutableRepository::invocation_snapshot(connection, invocation).await?;
            SchemaContractRepository::materialize_action_pair(connection, &target).await?;
            let body = if let Some(body) = present {
                body
            } else {
                // Revisions created by an unrelated edit must retain the actual
                // original source release and body, even after projection drift.
                let retained: Option<(Id, Value, String)> = sqlx::query_as(
                    "SELECT d.source_release,d.metadata,d.metadata_digest FROM managed_dispatch_metadata_revision d \
                     JOIN pack_runtime_policy_revision p ON p.id=d.policy_revision \
                     WHERE d.component_kind=$1 AND d.component_id=$2 AND d.metadata_digest=$3 AND p.pack=$4 \
                     AND EXISTS(SELECT 1 FROM jsonb_array_elements(p.policy->'invocations') entry \
                         WHERE (entry->>'source_id')::BIGINT=d.component_id AND entry->'binding'=$5 \
                         AND entry->>'dispatch_template_digest'=d.metadata_digest) ORDER BY d.policy_revision LIMIT 1 FOR SHARE OF d,p",
                ).bind(kind).bind(source).bind(hash).bind(stored.policy.owner.pack_id)
                    .bind(serde_json::to_value(&invocation.binding)?).fetch_optional(&mut *connection).await?;
                // Identical authored bodies may be redeployed from NEW code.
                // A matching old digest must not silently pin the old source
                // release when this exact source was projected by this release.
                let retained = if let Some(body) = retained {
                    let query = format!("SELECT managed_release FROM {table} WHERE id=$1 AND ref=$2 AND pack=$3 AND management_origin='pack' AND retired_at IS NULL FOR SHARE");
                    let projected_release = sqlx::query_scalar::<_, Option<Id>>(&query)
                        .bind(source)
                        .bind(reference)
                        .bind(stored.policy.owner.pack_id)
                        .fetch_optional(&mut *connection)
                        .await?
                        .flatten();
                    if projected_release == Some(stored.policy.release_id)
                        && body.0 != stored.policy.release_id
                    {
                        let projected = Self::source_metadata(
                            connection,
                            stored.policy.owner.pack_id,
                            kind,
                            reference,
                        )
                        .await?;
                        if digest(&projected).ok().as_deref() == Some(hash) {
                            None
                        } else {
                            Some(body)
                        }
                    } else {
                        Some(body)
                    }
                } else {
                    None
                };
                match retained {
                    Some(body) => body,
                    None => {
                        // This is a newly captured source, not a historical
                        // lookup. Recheck the exact structural tuple and code.
                        let query = format!("SELECT managed_release,{target_column},{target_ref_column} FROM {table} WHERE id=$1 AND ref=$2 AND pack=$3 AND management_origin='pack' AND retired_at IS NULL FOR SHARE");
                        let projection: Option<(Option<Id>, Option<Id>, String)> =
                            sqlx::query_as(&query)
                                .bind(source)
                                .bind(reference)
                                .bind(stored.policy.owner.pack_id)
                                .fetch_optional(&mut *connection)
                                .await?;
                        let (release, action, action_ref) = projection.ok_or_else(|| {
                            Error::validation(
                                "Dispatch source has no matching structural projection",
                            )
                        })?;
                        if action != Some(target.executable.action.id)
                            || action_ref != invocation.binding.action_ref()
                        {
                            return Err(Error::validation(
                                "Dispatch target differs from its captured executable",
                            ));
                        }
                        let release = release.ok_or_else(|| {
                            Error::validation("Dispatch source has no immutable code release")
                        })?;
                        let metadata = Self::source_metadata(
                            connection,
                            stored.policy.owner.pack_id,
                            kind,
                            reference,
                        )
                        .await?;
                        if kind == "rules" {
                            use crate::repositories::{trigger::TriggerRepository, FindById};
                            let trigger = TriggerRepository::find_by_id(
                                &mut *connection,
                                metadata["trigger"].as_i64().ok_or_else(|| {
                                    Error::validation("Dispatch trigger identity is unavailable")
                                })?,
                            )
                            .await?
                            .ok_or_else(|| Error::validation("Dispatch trigger is unavailable"))?;
                            if metadata["trigger_ref"].as_str() != Some(trigger.r#ref.as_str()) {
                                return Err(Error::validation(
                                    "Dispatch trigger differs from its structural source",
                                ));
                            }
                            crate::action_visibility::ensure_trigger_reference_allowed(
                                &trigger,
                                Some(&stored.policy.owner.pack_ref),
                                "rule",
                                reference,
                            )?;
                        }
                        let actual = digest(&metadata)
                            .map_err(|_| Error::validation("Invalid dispatch metadata digest"))?;
                        (release, metadata, actual)
                    }
                }
            };
            let (release, metadata, actual) = body;
            let owner: Option<Id> =
                sqlx::query_scalar("SELECT pack FROM pack_release WHERE id=$1 FOR SHARE")
                    .bind(release)
                    .fetch_optional(&mut *connection)
                    .await?;
            if actual != hash
                || digest(&metadata).ok().as_deref() != Some(hash)
                || owner != Some(stored.policy.owner.pack_id)
                || metadata[target_column].as_i64() != Some(target.executable.action.id)
                || metadata[target_ref_column].as_str() != Some(invocation.binding.action_ref())
                || kind == "rules"
                    && (metadata["rule_id"].as_i64() != Some(source)
                        || metadata["rule_ref"].as_str() != Some(reference.as_str())
                        || metadata["pack_ref"].as_str()
                            != Some(stored.policy.owner.pack_ref.as_str())
                        || metadata["managing_pack_id"].as_i64()
                            != Some(stored.policy.owner.pack_id))
            {
                return Err(Error::validation(
                    "Dispatch metadata differs from its captured source or target",
                ));
            }
            sqlx::query("INSERT INTO managed_dispatch_metadata_revision (policy_revision,component_kind,component_id,source_release,metadata,metadata_digest) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (policy_revision,component_kind,component_id) DO NOTHING")
                .bind(stored.id).bind(kind).bind(source).bind(release).bind(&metadata).bind(hash).execute(&mut *connection).await?;
            let exact: bool = sqlx::query_scalar("SELECT source_release=$4 AND metadata=$5 AND metadata_digest=$6 FROM managed_dispatch_metadata_revision WHERE policy_revision=$1 AND component_kind=$2 AND component_id=$3")
                .bind(stored.id).bind(kind).bind(source).bind(release).bind(metadata).bind(hash).fetch_one(&mut *connection).await?;
            if !exact {
                return Err(Error::validation("Conflicting immutable dispatch metadata"));
            }
            sqlx::query("INSERT INTO runtime_authority_release_dependency (policy_revision,release) VALUES ($1,$2) ON CONFLICT (policy_revision,release) DO NOTHING")
                .bind(stored.id).bind(release).execute(&mut *connection).await?;
        }
        Ok(())
    }
}
