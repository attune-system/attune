//! Server-owned selection and handoff of immutable invocation authority.
//!
//! Callers hold the transaction through execution/enforcement creation and attach
//! only when the row was inserted. A deduplicated row retains its original record.
use super::FindById;
use sqlx::PgConnection;
use uuid::Uuid;

use super::runtime_authority::{
    ApprovalEffectFence, RuntimeAdmissionRecord, RuntimeAuthorityRepository,
    RuntimeCredentialBinding, RuntimeOwner, ValidatedRuntimeAuthority,
};
use crate::{
    models::{ExecutionExecutableSnapshot, Id},
    runtime_authority::{InvocationBinding, RequestAttribution, SelectedRuntimeAuthority},
    Error, Result,
};

/// Cannot be constructed from caller-supplied grants or policy IDs.
pub struct ExecutionAdmission {
    authority: SelectedRuntimeAuthority,
    inherited_source: Option<(Id, RuntimeOwner)>,
    effect_fence: ApprovalEffectFence,
}

impl ExecutionAdmission {
    pub fn effect_fence(&self) -> &ApprovalEffectFence {
        &self.effect_fence
    }
    pub async fn ensure_fresh_on(&self, connection: &mut PgConnection) -> Result<()> {
        self.effect_fence.ensure_fresh_on(connection).await
    }

    pub fn authority(&self) -> &SelectedRuntimeAuthority {
        &self.authority
    }

    pub async fn attach(
        &self,
        connection: &mut PgConnection,
        owner: RuntimeOwner,
    ) -> Result<RuntimeAdmissionRecord> {
        let record =
            RuntimeAuthorityRepository::record_admission(connection, owner, &self.authority)
                .await?;
        if let Some((source, original)) = self.inherited_source {
            RuntimeAuthorityRepository::retain_inherited_admission(connection, source, record.id)
                .await?;
            if let (RuntimeOwner::Execution(source), RuntimeOwner::Execution(destination)) =
                (original, owner)
            {
                super::declared_input::DeclaredInputRepository::copy_deliveries(
                    connection,
                    source,
                    destination,
                )
                .await?;
            }
        }
        self.ensure_fresh_on(connection).await?;
        Ok(record)
    }
}

pub use super::execution_admission::ExecutionAdmissionRepository;

impl ExecutionAdmissionRepository {
    /// A recorded task's contract is keyed by its exact policy-resolved metadata,
    /// never a ref-only parent cache or current static-child discovery.
    pub async fn workflow_schema(
        connection: &mut PgConnection,
        parent: Id,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<super::schema_contract::ActionSchemaBinding> {
        if Self::find(connection, RuntimeOwner::Execution(parent))
            .await?
            .is_some()
        {
            Self::require(connection, RuntimeOwner::Execution(parent)).await?;
            super::schema_contract::SchemaContractRepository::materialize_action_pair(
                connection, snapshot,
            )
            .await
        } else {
            super::schema_contract::SchemaContractRepository::workflow_child_binding(
                connection, parent, snapshot,
            )
            .await
        }
    }
    /// Fresh action/rule/queue admission resolves an actual managing source and
    /// the immutable metadata revision selected by its current policy.
    pub async fn resolve_current_snapshot(
        connection: &mut PgConnection,
        sponsor_pack: Id,
        source: super::runtime_target::RuntimeTarget,
        binding: &InvocationBinding,
    ) -> Result<ExecutionExecutableSnapshot> {
        let policy = RuntimeAuthorityRepository::current_policy(connection, sponsor_pack)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Managed invocation policy is unavailable".into())
            })?;
        let source = super::runtime_target::RuntimeTargetRepository::resolve(
            connection,
            source,
            policy.policy.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(|| {
            Error::PermissionDenied("Managed invocation source is unavailable".into())
        })?;
        let invocation = policy.policy.invocation_for_source(binding, &source)?;
        Self::check_dispatch_source(connection, invocation, &source).await?;
        super::managed_executable::ManagedExecutableRepository::resolve_invocation(
            connection,
            &policy.policy,
            binding,
        )
        .await
    }

    /// Resolve from the parent's admitted graph, without current enablement or
    /// current policy lookup. Foreign graph handoffs remain fail-closed in A.
    pub async fn resolve_workflow_snapshot(
        connection: &mut PgConnection,
        parent: Id,
        task: &str,
        action_ref: &str,
    ) -> Result<ExecutionExecutableSnapshot> {
        let (policy, binding) =
            Self::workflow_invocation(connection, parent, task, action_ref).await?;
        super::managed_executable::ManagedExecutableRepository::resolve_invocation(
            connection,
            &policy.policy,
            &binding,
        )
        .await
    }

    pub(crate) async fn workflow_invocation(
        connection: &mut PgConnection,
        parent: Id,
        task: &str,
        action_ref: &str,
    ) -> Result<(
        super::runtime_authority::RuntimePolicyRevision,
        InvocationBinding,
    )> {
        let admitted = Self::require(connection, RuntimeOwner::Execution(parent)).await?;
        let execution = super::execution::ExecutionRepository::find_by_id(&mut *connection, parent)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Parent execution is unavailable".into()))?;
        let pinned = execution
            .executable_snapshot
            .as_ref()
            .ok_or_else(|| Error::PermissionDenied("Parent executable is unavailable".into()))?;
        let definition = pinned
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(|| {
                Error::PermissionDenied("Pinned workflow definition is unavailable".into())
            })?;
        let source_policy =
            RuntimeAuthorityRepository::require_policy(connection, admitted.policy_revision)
                .await?;
        let policy = super::workflow_policy::WorkflowPolicyRepository::for_invocation(
            connection,
            &source_policy,
            &admitted.authority.binding,
            pinned,
        )
        .await?;
        let bindings = policy
            .policy
            .invocations
            .iter()
            .filter(|invocation| match &invocation.binding {
                InvocationBinding::WorkflowTask {
                    workflow_ref,
                    task_path,
                    action_ref: target,
                } => {
                    workflow_ref == &pinned.executable.action.r#ref
                        && target == action_ref
                        && Self::workflow_task_matches(&definition.definition, task_path, task)
                }
                _ => false,
            })
            .map(|invocation| invocation.binding.clone())
            .collect::<Vec<_>>();
        let [binding] = bindings.as_slice() else {
            return Err(Error::PermissionDenied(
                "Workflow task has missing or ambiguous admitted identity".into(),
            ));
        };
        let binding = binding.clone();
        policy
            .policy
            .invocation_for_retained_workflow(&binding, pinned)?;
        Ok((policy, binding))
    }

    async fn check_policy_snapshot(
        connection: &mut PgConnection,
        authority: &SelectedRuntimeAuthority,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<()> {
        Self::check_target(authority, snapshot)?;
        let policy = RuntimeAuthorityRepository::find_policy(connection, authority.policy_id)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Executable policy is unavailable".into()))?;
        let expected = super::managed_executable::ManagedExecutableRepository::resolve_invocation(
            connection,
            &policy.policy,
            &authority.binding,
        )
        .await?;
        if serde_json::to_value(&expected.executable)?
            != serde_json::to_value(&snapshot.executable)?
        {
            return Err(Error::PermissionDenied(
                "Executable metadata differs from its immutable policy revision".into(),
            ));
        }
        Ok(())
    }

    async fn check_dispatch_source(
        connection: &mut PgConnection,
        invocation: &crate::runtime_authority::InvocationPolicy,
        source: &super::runtime_target::ResolvedRuntimeTarget,
    ) -> Result<()> {
        let component = match &invocation.binding {
            InvocationBinding::Queue { queue_ref, .. } => Some(("queues", queue_ref)),
            InvocationBinding::Rule { rule_ref, .. } => Some(("rules", rule_ref)),
            _ => None,
        };
        if let Some((kind, reference)) = component {
            let body =
                super::managed_component_readmission::ManagedComponentReadmission::source_metadata(
                    connection,
                    source.pack_id().ok_or_else(|| {
                        Error::PermissionDenied("Dispatch source ownership missing".into())
                    })?,
                    kind,
                    reference,
                )
                .await?;
            let digest = crate::value_schema::digest(&body)
                .map_err(|_| Error::validation("Invalid dispatch source digest"))?;
            if invocation.dispatch_template_digest.as_deref() != Some(digest.as_str()) {
                return Err(Error::PermissionDenied(
                    "Dispatch definition needs policy readmission before new acceptance".into(),
                ));
            }
        }
        Ok(())
    }

    /// Legacy ad-hoc delegation is a separate source. Fence its real row through
    /// child creation; an unbound token cannot impersonate a recorded owner.
    pub async fn validate_legacy_origin(
        connection: &mut PgConnection,
        execution: Id,
        identity: Id,
        action: &str,
        refs: &[String],
    ) -> Result<()> {
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        type ExecutionSelectionRow = (String, Option<Id>, String, Vec<String>, bool);
        let row: Option<ExecutionSelectionRow> = sqlx::query_as(
            "SELECT status::text,executor,action_ref,permission_set_refs,COALESCE((executable_snapshot->'executable'->'action'->>'is_adhoc')::boolean,FALSE) FROM execution WHERE id = $1 FOR SHARE",
        ).bind(execution).fetch_optional(&mut *connection).await?;
        let Some((status, owner, action_ref, selected, adhoc)) = row else {
            return Err(Error::PermissionDenied(
                "Originating execution is unavailable".into(),
            ));
        };
        if !adhoc
            || status != "running"
            || owner != Some(identity)
            || action_ref != action
            || refs.is_empty()
            || selected.iter().collect::<std::collections::BTreeSet<_>>()
                != refs.iter().collect::<std::collections::BTreeSet<_>>()
            || Self::find(connection, RuntimeOwner::Execution(execution))
                .await?
                .is_some()
        {
            return Err(Error::PermissionDenied(
                "Originating legacy credential does not match its active ad-hoc execution".into(),
            ));
        }
        Ok(())
    }

    pub async fn has_queue_item_admission(
        connection: &mut PgConnection,
        items: &[crate::models::WorkQueueItem],
    ) -> Result<bool> {
        let ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM work_queue_item_runtime_admission WHERE item = ANY($1))",
        )
        .bind(ids)
        .fetch_one(connection)
        .await?)
    }

    /// Native iteration reads the selected task invocation, not the parent's
    /// token refs or current set bodies. No worker credential/attempt is issued.
    pub async fn authorize_workflow_cache_read(
        connection: &mut PgConnection,
        admission: &ExecutionAdmission,
        namespace: Id,
    ) -> Result<bool> {
        admission.ensure_fresh_on(connection).await?;
        let authority = &admission.authority;
        let policy = RuntimeAuthorityRepository::find_policy(connection, authority.policy_id)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Native cache policy is unavailable".into()))?;
        let target = super::runtime_target::RuntimeTargetRepository::resolve(
            connection,
            super::runtime_target::RuntimeTarget::CacheNamespace(namespace),
            authority.owner.sponsor_identity_id,
        )
        .await?;
        let Some(target) = target else {
            return Ok(false);
        };
        if authority.grants.iter().any(|grant| {
            let Ok(key) = super::runtime_target_binding::grant_key(grant) else {
                return false;
            };
            let Some(binding) = policy.policy.concrete_targets.get(&key) else {
                return false;
            };
            super::runtime_target_binding::matches(binding, &target)
                && grant.allows(
                    &authority.approval_ids(),
                    target.pack_id(),
                    crate::rbac::Resource::Caches,
                    crate::rbac::Action::Read,
                    target.context(),
                )
        }) {
            return Ok(true);
        }
        if !authority
            .selected_refs
            .iter()
            .any(|reference| reference == "standard")
        {
            return Ok(false);
        }
        let owner_type = match target.context().owner_type {
            Some(crate::models::OwnerType::Pack) => "pack",
            Some(crate::models::OwnerType::Action) => "action",
            _ => return Ok(false),
        };
        let Some(owner_id) = target.owner_id() else {
            return Ok(false);
        };
        let invocation = crate::value_schema::digest(&serde_json::to_value(&authority.binding)?)
            .map_err(|_| Error::validation("Invalid native invocation"))?;
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_standard_owner_binding WHERE policy_revision = $1 AND invocation_digest = $2 AND owner_type = $3 AND owner_id = $4)")
            .bind(authority.policy_id).bind(invocation).bind(owner_type).bind(owner_id).fetch_one(connection).await?)
    }

    /// Worker/origin consumers must validate each contributor independently.
    /// No contributor's accepted rights authorize another item's input.
    pub async fn input_key_origin_allowed(
        connection: &mut PgConnection,
        execution: Id,
        origin: &crate::secret_provenance::KeyOrigin,
        action: crate::rbac::Action,
    ) -> Result<bool> {
        let authorities = Self::input_authorities(connection, execution, None).await?;
        if authorities.is_empty() {
            return Ok(false);
        }
        let ownership =
            super::iam_ownership::VerifiedKeyOrigins::load(&mut *connection, &[origin.key_id])
                .await?;
        Ok(authorities
            .iter()
            .all(|authority| authority.key_allows_verified(origin, action, &ownership)))
    }

    /// Every captured contributor must permit a delivered value. Callers must
    /// not union this vector into broader runtime or input grants.
    pub async fn input_authorities(
        connection: &mut PgConnection,
        execution: Id,
        identity: Option<Id>,
    ) -> Result<Vec<crate::delegation::DelegationAuthority>> {
        if Self::find(connection, RuntimeOwner::Execution(execution))
            .await?
            .is_some()
        {
            Self::require(connection, RuntimeOwner::Execution(execution)).await?;
            Self::require_input_liveness(connection, execution).await?;
            if let Some(authorities) = super::recorded_input_authority::RecordedInputAuthorityRepository::queue_authorities(connection, execution).await? {
                return Ok(authorities);
            }
        }
        Ok(Self::input_authority(connection, execution, identity)
            .await?
            .into_iter()
            .collect())
    }

    /// Called in the enqueue transaction after the item's immutable executable
    /// has been selected. Attribution is supplied by authenticated server code.
    pub async fn admit_queue_item(
        connection: &mut PgConnection,
        queue: &crate::models::WorkQueue,
        item: &crate::models::WorkQueueItem,
        actor: Option<crate::runtime_authority::HumanAttribution>,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<()> {
        if queue.is_adhoc {
            return Ok(());
        }
        if item.queue != queue.id
            || item.queue_ref != queue.r#ref
            || item.pack_release != Some(snapshot.release.id)
            || item.pack_release_digest.as_deref() != Some(snapshot.release.digest.as_str())
            || item.executable_snapshot.as_ref() != Some(&serde_json::to_value(snapshot)?)
        {
            return Err(Error::validation("Queue admission item mismatch"));
        }
        let refs = queue.permission_set_refs.as_deref().unwrap_or(
            &snapshot
                .executable
                .action
                .default_execution_permission_set_refs,
        );
        let admission = Self::select(
            connection,
            queue
                .pack
                .ok_or_else(|| Error::validation("Queue sponsor is unavailable"))?,
            InvocationBinding::Queue {
                queue_ref: queue.r#ref.clone(),
                action_ref: snapshot.executable.action.r#ref.clone(),
            },
            snapshot,
            refs,
            RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: std::collections::BTreeMap::from([(item.id, actor)]),
            },
            super::runtime_target::RuntimeTarget::Component(
                super::runtime_target::ComponentTarget::Queue,
                queue.id,
            ),
        )
        .await?;
        let authority = &admission.authority;
        let digest = crate::value_schema::digest(&serde_json::to_value(authority)?)
            .map_err(|_| Error::validation("Invalid queue authority"))?;
        sqlx::query("INSERT INTO work_queue_item_runtime_admission (item,policy_revision,authority,authority_digest) VALUES ($1,$2,$3,$4)")
            .bind(item.id).bind(authority.policy_id).bind(sqlx::types::Json(&authority)).bind(digest).execute(&mut *connection).await?;
        admission.ensure_fresh_on(connection).await?;
        Ok(())
    }

    /// A batch uses accepted records, never a current policy or the first
    /// contributor's identity. Differing requesters/grants do not change leasing.
    pub async fn queue_batch(
        connection: &mut PgConnection,
        queue: &crate::models::WorkQueue,
        items: &[crate::models::WorkQueueItem],
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<ExecutionAdmission> {
        let mut selected: Option<SelectedRuntimeAuthority> = None;
        let mut effect_fence: Option<ApprovalEffectFence> = None;
        let mut attribution = std::collections::BTreeMap::new();
        if queue.is_adhoc {
            super::ad_hoc_queue_admission::AdHocQueueAdmissionRepository::validate_shape(queue)?;
            if items.is_empty()
                || items.len() > 256
                || (queue.batch_mode == crate::models::WorkQueueBatchMode::Single
                    && items.len() != 1)
            {
                return Err(Error::PermissionDenied(
                    "Ad-hoc dispatch item set differs from its accepted batch shape".into(),
                ));
            }
            let pinned = super::ad_hoc_queue_admission::AdHocQueueAdmissionRepository::template(
                connection, queue, items,
            )
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Accepted ad-hoc template unavailable".into())
            })?;
            if serde_json::to_value(&pinned)? != serde_json::to_value(queue)? {
                return Err(Error::PermissionDenied(
                    "Dispatch must consume the accepted ad-hoc template".into(),
                ));
            }
        }
        for item in items {
            let record: Option<(i64, sqlx::types::Json<SelectedRuntimeAuthority>, String)> = sqlx::query_as(
                "SELECT policy_revision,authority,authority_digest FROM work_queue_item_runtime_admission WHERE item = $1 FOR SHARE",
            ).bind(item.id).fetch_optional(&mut *connection).await?;
            let (revision, authority, digest) = record.ok_or_else(|| {
                Error::PermissionDenied(
                    "Queue item has no accepted runtime admission; readmission is required".into(),
                )
            })?;
            if revision != authority.policy_id
                || crate::value_schema::digest(&serde_json::to_value(&authority.0)?)
                    .map_err(|_| Error::validation("Invalid queue authority"))?
                    != digest
            {
                return Err(Error::validation("Queue authority digest mismatch"));
            }
            Self::validate_selected_policy(connection, &authority).await?;
            let mut next_fence =
                RuntimeAuthorityRepository::selected_effect_fence(connection, &authority).await?;
            if queue.is_adhoc {
                next_fence = next_fence.intersect(
                    &super::ad_hoc_queue_admission::AdHocQueueAdmissionRepository::source_effect_fence(connection, item.id).await?,
                );
            }
            effect_fence = Some(match effect_fence {
                Some(fence) => fence.intersect(&next_fence),
                None => next_fence,
            });
            let policy = RuntimeAuthorityRepository::find_policy(connection, authority.policy_id)
                .await?
                .ok_or_else(|| {
                    Error::PermissionDenied("Accepted queue policy unavailable".into())
                })?;
            let (source_kind, source_id) = if queue.is_adhoc {
                (
                    super::runtime_target::ComponentTarget::Action,
                    snapshot.executable.action.id,
                )
            } else {
                (super::runtime_target::ComponentTarget::Queue, queue.id)
            };
            let source = super::runtime_target::RuntimeTargetRepository::resolve(
                connection,
                super::runtime_target::RuntimeTarget::Component(source_kind, source_id),
                authority.owner.sponsor_identity_id,
            )
            .await?
            .ok_or_else(|| Error::PermissionDenied("Accepted queue source unavailable".into()))?;
            if source.pack_id() != Some(authority.owner.pack_id)
                || !policy.policy.invocations.iter().any(|entry| {
                    entry.binding == authority.binding && entry.source_id == Some(source_id)
                })
            {
                return Err(Error::PermissionDenied(
                    "Accepted queue source identity changed".into(),
                ));
            }
            Self::check_policy_snapshot(connection, &authority, snapshot).await?;
            match &authority.attribution {
                RequestAttribution::ParentExecution { .. } if queue.is_adhoc => {
                    // The retained accepted-item manifest keeps the actual
                    // source execution admission and its historical attribution.
                    // Do not manufacture a human owner for a batch contributor.
                    attribution.insert(item.id, None);
                }
                RequestAttribution::QueueBatch {
                    queue_ref,
                    items: actors,
                } if queue_ref == &queue.r#ref
                    && actors.len() == 1
                    && actors.contains_key(&item.id) =>
                {
                    attribution.insert(item.id, actors[&item.id].clone());
                }
                _ => return Err(Error::validation("Accepted queue attribution mismatch")),
            }
            let expected_binding = if queue.is_adhoc {
                InvocationBinding::Action {
                    action_ref: snapshot.executable.action.r#ref.clone(),
                }
            } else {
                InvocationBinding::Queue {
                    queue_ref: queue.r#ref.clone(),
                    action_ref: snapshot.executable.action.r#ref.clone(),
                }
            };
            if item.queue != queue.id || authority.binding != expected_binding {
                return Err(Error::validation("Accepted queue invocation mismatch"));
            }
            if let Some(first) = &selected {
                if (!queue.is_adhoc && first.policy_id != authority.policy_id)
                    || first.owner != authority.owner
                    || first.binding != authority.binding
                    || first.selected_refs != authority.selected_refs
                    || first.grants != authority.grants
                {
                    return Err(Error::PermissionDenied("Batch spans incompatible admitted queue revisions; drain before changing invocation authority".into()));
                }
            } else {
                selected = Some(authority.0);
            }
        }
        let mut authority = selected.ok_or_else(|| Error::validation("Queue batch is empty"))?;
        authority.attempt = Uuid::new_v4();
        if !queue.is_adhoc || queue.batch_mode == crate::models::WorkQueueBatchMode::Batch {
            authority.attribution = RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: attribution,
            };
        }
        Ok(ExecutionAdmission {
            authority,
            inherited_source: None,
            effect_fence: effect_fence.ok_or_else(|| Error::validation("Queue batch is empty"))?,
        })
    }

    async fn validate_selected_policy(
        connection: &mut PgConnection,
        authority: &SelectedRuntimeAuthority,
    ) -> Result<()> {
        let policy =
            RuntimeAuthorityRepository::require_policy(connection, authority.policy_id).await?;
        authority.validate_against(&policy.policy)?;
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let owner = &authority.owner;
        let pack: Option<String> =
            sqlx::query_scalar("SELECT ref FROM pack WHERE id = $1 FOR SHARE")
                .bind(owner.pack_id)
                .fetch_optional(&mut *connection)
                .await?;
        let sponsor: Option<Id> = sqlx::query_scalar(
            "SELECT identity FROM pack_runtime_identity WHERE pack = $1 FOR SHARE",
        )
        .bind(owner.pack_id)
        .fetch_optional(&mut *connection)
        .await?;
        if pack.as_deref() != Some(owner.pack_ref.as_str())
            || sponsor != Some(owner.sponsor_identity_id)
            || super::identity::IdentityRepository::find_by_id_for_share(
                connection,
                owner.sponsor_identity_id,
            )
            .await?
            .is_none_or(|identity| identity.frozen)
        {
            return Err(Error::PermissionDenied(
                "Accepted queue sponsor is unavailable".into(),
            ));
        }
        let _: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM pack_runtime_policy_revision WHERE id = $1 FOR SHARE",
        )
        .bind(authority.policy_id)
        .fetch_optional(&mut *connection)
        .await?;
        let policy = RuntimeAuthorityRepository::find_policy(connection, authority.policy_id)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Accepted queue policy is unavailable".into())
            })?;
        authority.validate_against(&policy.policy)?;
        if policy.revoked_at.is_some() {
            return Err(Error::PermissionDenied(
                "Accepted queue policy was revoked".into(),
            ));
        }
        for id in authority.approval_ids() {
            let _: Option<Id> = sqlx::query_scalar(
                "SELECT id FROM runtime_external_approval WHERE id = $1 FOR SHARE",
            )
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?;
        }
        for grant in &authority.grants {
            if let crate::runtime_authority::AdmittedGrant::External { approval_id, grant } = grant
            {
                let approval = RuntimeAuthorityRepository::find_approval(connection, *approval_id)
                    .await?
                    .ok_or_else(|| {
                        Error::PermissionDenied("Accepted queue approval is unavailable".into())
                    })?;
                if !approval.is_active(chrono::Utc::now())
                    || !approval.approval()?.covers(&authority.owner, grant)
                {
                    return Err(Error::PermissionDenied(
                        "Accepted queue approval is not active".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Purpose-specific input grants are not the runtime API selection. A
    /// recorded execution may never borrow the requester's current IAM state.
    pub async fn input_authority(
        connection: &mut PgConnection,
        execution: Id,
        identity: Option<Id>,
    ) -> Result<Option<crate::delegation::DelegationAuthority>> {
        if Self::find(connection, RuntimeOwner::Execution(execution))
            .await?
            .is_some()
        {
            Self::require(connection, RuntimeOwner::Execution(execution)).await?;
            Self::require_input_liveness(connection, execution).await?;
            if let Some(authority) =
                super::recorded_input_authority::RecordedInputAuthorityRepository::load(
                    connection,
                    "execution",
                    execution,
                )
                .await?
            {
                return Ok(Some(authority));
            }
            return match super::rule_authority::RuleAuthorityRepository::runtime(
                connection,
                "execution",
                execution,
            )
            .await?
            {
                Some(snapshot) => Ok(Some(snapshot.input_authority(connection).await?)),
                None => Ok(None),
            };
        }
        let row = super::execution::ExecutionRepository::find_by_id(&mut *connection, execution)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Input execution is unavailable".into()))?;
        if row
            .executable_snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.executable.action.is_adhoc)
        {
            return Err(Error::PermissionDenied(
                "Managed execution input admission is unavailable".into(),
            ));
        }
        super::rule_authority::RuleAuthorityRepository::execution_input_authority(
            connection, execution, identity,
        )
        .await
    }

    /// Production workflow persistence boundary. The caller owns the transaction
    /// and schema/input persistence. Cache completion sentinels have no effects.
    pub async fn create_workflow_task_if_absent_pinned_with_conn(
        connection: &mut PgConnection,
        mut input: super::execution::CreateExecutionInput,
        snapshot: &ExecutionExecutableSnapshot,
        workflow: Id,
        task: &str,
        index: Option<i32>,
    ) -> Result<super::execution::WorkflowTaskExecutionCreateOrGetResult> {
        let parent = input.parent;
        let source_closure = if let Some(parent) = parent {
            if Self::find(connection, RuntimeOwner::Execution(parent))
                .await?
                .is_some()
            {
                Some(
                    super::native_input_origin::NativeInputOriginRepository::capture(
                        connection,
                        parent,
                        &[],
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let workflow_parent: Option<Id> =
            sqlx::query_scalar("SELECT execution FROM workflow_execution WHERE id = $1")
                .bind(workflow)
                .fetch_optional(&mut *connection)
                .await?;
        if workflow_parent != parent || parent.is_none() {
            return Err(Error::PermissionDenied(
                "Workflow task parent does not match its workflow".into(),
            ));
        }
        if source_closure.is_some() {
            let running: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM execution WHERE id = $1 AND status = 'running')",
            )
            .bind(parent)
            .fetch_one(&mut *connection)
            .await?;
            if !running {
                return Err(Error::PermissionDenied(
                    "Parent execution no longer permits task effects".into(),
                ));
            }
        }
        if let Some(existing) =
            super::execution::ExecutionRepository::reserve_workflow_task_with_conn(
                connection, workflow, task, index,
            )
            .await?
        {
            let metadata_matches = existing.workflow_task.as_ref().is_some_and(|metadata| {
                metadata.workflow_execution == workflow
                    && metadata.task_name == task
                    && metadata.task_index == index
            });
            if existing.parent != parent
                || existing.action != input.action
                || existing.action_ref != input.action_ref
                || !metadata_matches
                || serde_json::to_value(&existing.executable_snapshot)?
                    != serde_json::to_value(Some(snapshot))?
            {
                return Err(Error::PermissionDenied(
                    "Existing workflow task has different immutable lineage or executable".into(),
                ));
            }
            if (source_closure.is_some()
                && !matches!(
                    existing.status,
                    crate::models::ExecutionStatus::Completed
                        | crate::models::ExecutionStatus::Failed
                ))
                || Self::find(connection, RuntimeOwner::Execution(existing.id))
                    .await?
                    .is_some()
            {
                let retained =
                    Self::require(connection, RuntimeOwner::Execution(existing.id)).await?;
                if retained.authority.attribution
                    != (RequestAttribution::ParentExecution {
                        execution_id: parent.unwrap(),
                    })
                {
                    return Err(Error::PermissionDenied(
                        "Existing task admission has different parent attribution".into(),
                    ));
                }
                let (_, binding) = Self::workflow_invocation(
                    connection,
                    parent.unwrap(),
                    task,
                    &existing.action_ref,
                )
                .await?;
                if retained.authority.binding != binding {
                    return Err(Error::PermissionDenied(
                        "Existing task admission has different authored lineage".into(),
                    ));
                }
                Self::check_policy_snapshot(connection, &retained.authority, snapshot).await?;
                RuntimeAuthorityRepository::retained_admission_fence(connection, retained.id)
                    .await?
                    .ensure_fresh_on(connection)
                    .await?;
            }
            if let Some(closure) = source_closure {
                closure.ensure_fresh_on(connection).await?;
            }
            return Ok(super::execution::WorkflowTaskExecutionCreateOrGetResult {
                execution: existing,
                created: false,
            });
        }
        let refs = input.permission_set_refs.clone();
        let identity = input.executor;
        let terminal = matches!(
            input.status,
            crate::models::enums::ExecutionStatus::Completed
                | crate::models::enums::ExecutionStatus::Failed
        );
        if terminal && !refs.is_empty() {
            return Err(Error::validation(
                "Terminal cache sentinel must be credential-free",
            ));
        }
        // Select/resolve immutable policy metadata before any new task row is
        // persisted. Create-if-absent still attaches only to a newly inserted row.
        let admission = if terminal {
            None
        } else {
            let retained = if let Some(parent) = parent {
                Self::find(connection, RuntimeOwner::Execution(parent))
                    .await?
                    .is_some()
            } else {
                false
            };
            let managed_parent = if let Some(parent) = parent {
                super::execution::ExecutionRepository::find_by_id(&mut *connection, parent)
                    .await?
                    .and_then(|execution| execution.executable_snapshot)
                    .is_some_and(|snapshot| !snapshot.executable.action.is_adhoc)
            } else {
                false
            };
            if retained || managed_parent || !snapshot.executable.action.is_adhoc {
                Some(
                    Self::workflow_child_prepared(
                        connection,
                        parent
                            .ok_or_else(|| Error::validation("Workflow child requires parent"))?,
                        task,
                        snapshot,
                        &refs,
                        source_closure.as_ref().ok_or_else(|| {
                            Error::PermissionDenied("Workflow source admission unavailable".into())
                        })?,
                    )
                    .await?,
                )
            } else {
                crate::delegation::require_execution_refs_for_share(connection, identity, &refs)
                    .await?;
                None
            }
        };
        if admission.is_some() {
            // Supplied values remain in the renderer until one-shot initialization.
            input.config = None;
        }
        let result =
            super::execution::ExecutionRepository::create_workflow_task_if_absent_pinned_with_conn(
                connection, input, snapshot, workflow, task, index,
            )
            .await?;
        if result.created && !terminal {
            if let Some(admission) = admission {
                admission
                    .attach(connection, RuntimeOwner::Execution(result.execution.id))
                    .await?;
                super::rule_authority::RuleAuthorityRepository::copy_execution(
                    connection,
                    parent.ok_or_else(|| Error::validation("Workflow child requires parent"))?,
                    result.execution.id,
                )
                .await?;
                super::recorded_input_authority::RecordedInputAuthorityRepository::copy(
                    connection,
                    "execution",
                    parent.ok_or_else(|| Error::validation("Workflow child requires parent"))?,
                    "execution",
                    result.execution.id,
                )
                .await?;
            }
        }
        Ok(result)
    }

    /// New invocations select the active sponsor policy. The executable must be
    /// the already selected immutable target, never a fresh lookup by ref.
    pub async fn select(
        connection: &mut PgConnection,
        sponsor_pack: Id,
        binding: InvocationBinding,
        snapshot: &ExecutionExecutableSnapshot,
        refs: &[String],
        attribution: RequestAttribution,
        source: super::runtime_target::RuntimeTarget,
    ) -> Result<ExecutionAdmission> {
        let policy = RuntimeAuthorityRepository::current_policy(connection, sponsor_pack)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied(
                    "Managed invocation has no admitted policy; redeploy its pack".into(),
                )
            })?;
        let authority = policy
            .policy
            .select(policy.id, &binding, refs, attribution)?;
        let effect_fence =
            RuntimeAuthorityRepository::selected_effect_fence(connection, &authority).await?;
        let source = super::runtime_target::RuntimeTargetRepository::resolve(
            connection,
            source,
            policy.policy.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(|| Error::PermissionDenied("Invocation source is unavailable".into()))?;
        let invocation = policy.policy.invocation_for_source(&binding, &source)?;
        Self::check_dispatch_source(connection, invocation, &source).await?;
        Self::check_policy_snapshot(connection, &authority, snapshot).await?;
        let actions = super::action::ActionRepository::find_by_refs_for_share(
            connection,
            std::slice::from_ref(&snapshot.executable.action.r#ref),
        )
        .await?;
        if !actions.iter().any(|action| {
            action.id == snapshot.executable.action.id
                && action.pack == snapshot.executable.action.pack
                && action.enabled
        }) {
            return Err(Error::PermissionDenied(
                "Action is unavailable for new invocation".into(),
            ));
        }
        Self::validate_selected_policy(connection, &authority).await?;
        Ok(ExecutionAdmission {
            authority,
            inherited_source: None,
            effect_fence,
        })
    }

    /// Workflow children use the parent's policy revision, not current policy or
    /// copied parent refs. Rendered refs must fit the specific task ceiling.
    pub async fn workflow_child(
        connection: &mut PgConnection,
        parent: Id,
        task: &str,
        snapshot: &ExecutionExecutableSnapshot,
        refs: &[String],
    ) -> Result<ExecutionAdmission> {
        let native_closure = super::native_input_origin::NativeInputOriginRepository::capture(
            connection,
            parent,
            &[],
        )
        .await?;
        Self::workflow_child_prepared(connection, parent, task, snapshot, refs, &native_closure)
            .await
    }

    pub(super) async fn workflow_child_prepared(
        connection: &mut PgConnection,
        parent: Id,
        task: &str,
        snapshot: &ExecutionExecutableSnapshot,
        refs: &[String],
        native_closure: &super::native_input_origin::NativeInputClosure,
    ) -> Result<ExecutionAdmission> {
        let parent_execution =
            super::execution::ExecutionRepository::find_by_id(&mut *connection, parent)
                .await?
                .ok_or_else(|| Error::PermissionDenied("Parent execution is unavailable".into()))?;
        if parent_execution.status != crate::models::ExecutionStatus::Running {
            return Err(Error::PermissionDenied(
                "Parent execution no longer permits child creation".into(),
            ));
        }
        let (policy, binding) =
            Self::workflow_invocation(connection, parent, task, &snapshot.executable.action.r#ref)
                .await?;
        let authority = policy.policy.select(
            policy.id,
            &binding,
            refs,
            RequestAttribution::ParentExecution {
                execution_id: parent,
            },
        )?;
        let effect_fence =
            RuntimeAuthorityRepository::selected_effect_fence(connection, &authority)
                .await?
                .intersect(native_closure.effect_fence());
        Self::check_policy_snapshot(connection, &authority, snapshot).await?;
        Self::validate_selected_policy(connection, &authority).await?;
        Ok(ExecutionAdmission {
            authority,
            inherited_source: None,
            effect_fence,
        })
    }

    /// Handoff/retry revalidates operational revocation, never human IAM or
    /// current action enablement. Each new execution gets its own attempt.
    pub async fn inherit(
        connection: &mut PgConnection,
        source: RuntimeOwner,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<ExecutionAdmission> {
        let record = Self::require(connection, source).await?;
        let retained_fence =
            RuntimeAuthorityRepository::retained_admission_fence(connection, record.id).await?;
        if let RuntimeOwner::Execution(id) = source {
            let execution = super::execution::ExecutionRepository::find_by_id(&mut *connection, id)
                .await?
                .ok_or_else(|| Error::PermissionDenied("Retry source is unavailable".into()))?;
            if matches!(
                execution.status,
                crate::models::ExecutionStatus::Canceling
                    | crate::models::ExecutionStatus::Cancelled
                    | crate::models::ExecutionStatus::Abandoned
            ) {
                return Err(Error::PermissionDenied(
                    "Cancelled execution cannot admit retries".into(),
                ));
            }
        }
        let mut authority = record.authority.0;
        Self::check_policy_snapshot(connection, &authority, snapshot).await?;
        authority.attempt = Uuid::new_v4();
        let effect_fence =
            RuntimeAuthorityRepository::selected_effect_fence(connection, &authority)
                .await?
                .intersect(&retained_fence);
        Ok(ExecutionAdmission {
            authority,
            inherited_source: Some((record.id, source)),
            effect_fence,
        })
    }

    pub async fn find(
        connection: &mut PgConnection,
        owner: RuntimeOwner,
    ) -> Result<Option<RuntimeAdmissionRecord>> {
        let (kind, id) = match owner {
            RuntimeOwner::Execution(id) => ("execution", id),
            RuntimeOwner::Enforcement(id) => ("enforcement", id),
        };
        // Repository-owned query; no caller may manufacture a retained binding.
        Ok(sqlx::query_as(
            "SELECT id, owner_kind, owner_id, policy_revision, authority, authority_digest, revoked_at \
             FROM runtime_authority_admission WHERE owner_kind = $1 AND owner_id = $2",
        ).bind(kind).bind(id).fetch_optional(connection).await?)
    }

    pub async fn require(
        connection: &mut PgConnection,
        owner: RuntimeOwner,
    ) -> Result<RuntimeAdmissionRecord> {
        let record = Self::find(connection, owner).await?.ok_or_else(|| {
            Error::PermissionDenied("Recorded invocation authority is unavailable".into())
        })?;
        // Reuse the canonical lock/digest/sponsor/approval/revocation fence.
        RuntimeAuthorityRepository::record_admission(connection, owner, &record.authority).await
    }

    /// Native effects under the executing action's nonempty selection use its
    /// real current fence. No worker credential is issued or attempt rotated.
    /// Task-scoped reads use `authorize_workflow_cache_read` instead.
    pub async fn native_authority(
        connection: &mut PgConnection,
        execution: Id,
    ) -> Result<ValidatedRuntimeAuthority> {
        let record = Self::require(connection, RuntimeOwner::Execution(execution)).await?;
        let attempt = sqlx::query_scalar(
            "SELECT attempt FROM runtime_authority_execution_fence WHERE execution = $1 AND admission = $2",
        ).bind(execution).bind(record.id).fetch_one(&mut *connection).await?;
        RuntimeAuthorityRepository::validate_execution_credential(
            connection,
            &RuntimeCredentialBinding {
                execution_id: execution,
                admission_id: record.id,
                sponsor_identity_id: record.authority.owner.sponsor_identity_id,
                action_ref: record.authority.binding.action_ref().into(),
                attempt,
            },
        )
        .await
    }

    fn check_target(
        authority: &SelectedRuntimeAuthority,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<()> {
        if authority.binding.action_ref() != snapshot.executable.action.r#ref
            || authority.target_release_id != snapshot.release.id
            || authority.target_release_digest != snapshot.release.digest
        {
            return Err(Error::PermissionDenied(
                "Invocation does not match its pinned executable".into(),
            ));
        }
        Ok(())
    }

    /// The planner retains authored JSON pointers. The persisted workflow parser
    /// normalizes named task maps to arrays, so resolve a selector by its actual
    /// index or its retained task name, never by an invented policy path.
    pub(super) fn workflow_task_matches(
        definition: &serde_json::Value,
        path: &str,
        name: &str,
    ) -> bool {
        let tokens = path.split('/').skip(1).collect::<Vec<_>>();
        if tokens.is_empty() || tokens.len() % 2 != 0 {
            return false;
        }
        let mut nodes = vec![(definition, None::<String>)];
        for pair in tokens.as_chunks::<2>().0 {
            if pair[0] != "tasks" {
                return false;
            }
            let selector = pair[1].replace("~1", "/").replace("~0", "~");
            let mut next = Vec::new();
            for (node, _) in nodes {
                match node.get("tasks") {
                    Some(serde_json::Value::Array(tasks)) => {
                        let mut matched = None;
                        for (index, task) in tasks.iter().enumerate() {
                            if index.to_string() == selector
                                || task.get("name").and_then(serde_json::Value::as_str)
                                    == Some(selector.as_str())
                            {
                                if matched.is_some() {
                                    return false;
                                }
                                matched = Some(task);
                            }
                        }
                        if let Some(task) = matched {
                            next.push((task, None));
                        }
                    }
                    Some(serde_json::Value::Object(tasks)) => {
                        if let Some(task) = tasks.get(&selector) {
                            next.push((task, Some(selector.clone())));
                        }
                    }
                    _ => {}
                }
            }
            nodes = next;
        }
        nodes.into_iter().any(|(task, implicit)| {
            task.get("name")
                .and_then(serde_json::Value::as_str)
                .or(implicit.as_deref())
                == Some(name)
        })
    }

    async fn require_input_liveness(connection: &mut PgConnection, execution: Id) -> Result<()> {
        let row = super::execution::ExecutionRepository::find_by_id(&mut *connection, execution)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Input execution is unavailable".into()))?;
        if !matches!(
            row.status,
            crate::models::ExecutionStatus::Requested
                | crate::models::ExecutionStatus::Scheduling
                | crate::models::ExecutionStatus::Scheduled
                | crate::models::ExecutionStatus::Running
        ) {
            return Err(Error::PermissionDenied(
                "Execution no longer permits input delivery".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ExecutionAdmissionRepository;
    use serde_json::json;

    #[test]
    fn task_binding_resolves_authored_array_identity() {
        let definition = json!({"tasks": [{"name": "first"}, {"name": "next"}]});
        assert!(ExecutionAdmissionRepository::workflow_task_matches(
            &definition,
            "/tasks/1",
            "next"
        ));
        assert!(!ExecutionAdmissionRepository::workflow_task_matches(
            &definition,
            "/tasks/0",
            "next"
        ));
    }

    #[test]
    fn task_binding_resolves_normalized_named_and_nested_identity() {
        let definition = json!({"tasks": [{"name": "group/name", "tasks": [{"name": "leaf"}]}]});
        assert!(ExecutionAdmissionRepository::workflow_task_matches(
            &definition,
            "/tasks/group~1name/tasks/0",
            "leaf"
        ));
        assert!(!ExecutionAdmissionRepository::workflow_task_matches(
            &definition,
            "/tasks/other/tasks/0",
            "leaf"
        ));
    }

    #[test]
    fn task_binding_rejects_unrelated_pointer_structure() {
        let definition = json!({"tasks": {"next": {"action": "pack.leaf"}}});
        assert!(ExecutionAdmissionRepository::workflow_task_matches(
            &definition,
            "/tasks/next",
            "next"
        ));
        assert!(!ExecutionAdmissionRepository::workflow_task_matches(
            &definition,
            "/variables/next",
            "next"
        ));
        assert!(!ExecutionAdmissionRepository::workflow_task_matches(
            &definition,
            "/tasks/next/action",
            "next"
        ));
        let ambiguous = json!({"tasks": [{"name": "first"}, {"name": "0"}]});
        assert!(!ExecutionAdmissionRepository::workflow_task_matches(
            &ambiguous, "/tasks/0", "0"
        ));
        assert!(!ExecutionAdmissionRepository::workflow_task_matches(
            &ambiguous, "/tasks/0", "first"
        ));
    }
}
