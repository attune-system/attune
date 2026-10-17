//! Read-only inquiry/queue-result targets from held authenticated native sources.
use super::*;
use crate::repositories::{
    native_input_disclosure::PreparedWaitSource, runtime_read::RuntimeReadOwner,
};

fn denied() -> Error {
    Error::PermissionDenied("Native wait source owner is unavailable".into())
}

impl RuntimeTargetRepository {
    pub async fn resolve_related_wait_source(
        conn: &mut PgConnection,
        source: &PreparedWaitSource,
        principal: Id,
    ) -> Result<ResolvedRuntimeTarget> {
        source.ensure_held_on(conn).await?;
        let metadata = source.metadata();
        let binding = metadata.target();
        let (kind, resource, current) = match (metadata.source_kind(), binding.kind.as_str()) {
            ("inquiry_response", "inquiry") => (
                "inquiry",
                Resource::Inquiries,
                RuntimeTarget::Inquiry(binding.id),
            ),
            ("queue_item_result", "work_queue_item") => (
                "work_queue_item",
                Resource::QueueItems,
                RuntimeTarget::QueueItem(binding.id),
            ),
            _ => return Err(denied()),
        };
        let reference = metadata.read_owner().related_wait_reference(binding)?;
        let mut context = AuthorizationContext::new(principal);
        context.target_id = Some(binding.id);
        context.target_ref = reference.map(str::to_owned);
        context.managing_pack_id = binding.pack_id;
        if let Some(pack) = binding.pack_id {
            context.pack_ref = Some(pack_ref(conn, pack).await?.ok_or_else(denied)?);
        }
        let resolved = ResolvedRuntimeTarget {
            kind,
            resource,
            pack_id: binding.pack_id,
            owner_id: None,
            context,
            accepts_new_effects: false,
        };
        if RuntimeReadOwner::of(&resolved) != *metadata.read_owner() {
            return Err(denied());
        }
        let live = if kind == "inquiry" {
            // The native source admission belongs to the actual creator, not
            // the inquiry ID. Its immutable release survives raw creator GC.
            let release = super::super::pack_release::PackReleaseRepository::find_by_id(
                &mut *conn,
                source.release(),
            )
            .await?
            .ok_or_else(denied)?;
            if release.digest != source.release_digest()
                || Some(release.pack) != binding.pack_id
                || resolved.context().pack_ref.as_deref() != Some(release.pack_ref.as_str())
            {
                return Err(denied());
            }
            let creator: Option<Option<Id>> = sqlx::query_scalar(
                "SELECT created_by_execution FROM inquiry WHERE id=$1 FOR SHARE",
            )
            .bind(binding.id)
            .fetch_optional(&mut *conn)
            .await?;
            if creator.is_some_and(|creator| creator != Some(source.source_execution())) {
                return Err(denied());
            }
            let actual: Option<(Option<Id>, Option<String>, String)> = sqlx::query_as("SELECT pack_release,pack_release_digest,action_ref FROM execution WHERE id=$1 FOR SHARE")
                .bind(source.source_execution()).fetch_optional(&mut *conn).await?;
            if let Some((release, digest, reference)) = &actual {
                if *release != Some(source.release())
                    || digest.as_deref() != Some(source.release_digest())
                    || reference.as_str() != source.action_ref()
                {
                    return Err(denied());
                }
            }
            // A surviving inquiry with a collected creator still has its exact
            // creator association above. Do not rebind it to a current owner.
            creator.is_some() && actual.is_some()
        } else {
            let item: Option<(String, Option<Id>)> = sqlx::query_as("SELECT queue_ref,requested_by_execution FROM work_queue_item WHERE id=$1 FOR SHARE")
                .bind(binding.id).fetch_optional(&mut *conn).await?;
            if let Some((queue_ref, requester)) = &item {
                if Some(queue_ref.as_str()) != reference
                    || *requester != Some(source.source_execution())
                {
                    return Err(denied());
                }
            }
            item.is_some()
        };
        if live {
            let target = Self::resolve(conn, current, principal)
                .await?
                .ok_or_else(denied)?;
            if RuntimeReadOwner::of(&target) != *metadata.read_owner() {
                return Err(denied());
            }
        }
        source.ensure_fresh()?;
        Ok(resolved)
    }
}
