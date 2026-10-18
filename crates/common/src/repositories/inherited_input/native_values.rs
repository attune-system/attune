//! Finite native VALUE recipes have their own proof discriminator. They cannot
//! be replayed as literals, accepted inputs or fabricated authored declarations.
use super::super::native_failure_materialization::PreparedNativeFailure;
use super::super::native_input_snapshot::{
    NativeInputSnapshotRepository as Snapshots, PreparedNativeCompletionMetadata,
    PreparedNativeResultView,
};
use super::super::FindById;
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "recipe", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum NativeValueRecipe {
    WaitCompletion {
        snapshot: Id,
    },
    OrdinarySuccess {
        snapshot: Id,
    },
    OrdinaryFailure {
        snapshot: Id,
    },
    ResultShorthand {
        snapshot: Id,
        directive_path: String,
        firing_control: Option<Id>,
    },
}
impl NativeValueRecipe {
    pub(super) fn snapshot(&self) -> Id {
        match self {
            Self::WaitCompletion { snapshot } => *snapshot,
            Self::OrdinarySuccess { snapshot } => *snapshot,
            Self::OrdinaryFailure { snapshot } => *snapshot,
            Self::ResultShorthand { snapshot, .. } => *snapshot,
        }
    }
}

pub struct PreparedNativeRecipeDestination {
    pub(super) rendered: RenderedReceiptContext,
    pub(super) destination: PreparedReceiptDestination,
    target_version: Value,
    pub(super) workflow_original: Option<Value>,
    workflow_checked: std::sync::OnceLock<()>,
}
impl PreparedNativeRecipeDestination {
    pub fn require_original_workflow(
        &self,
        actual: &crate::models::WorkflowExecution,
    ) -> Result<()> {
        if self.workflow_original.as_ref() != Some(&serde_json::to_value(actual)?) {
            return Err(denied());
        }
        self.workflow_checked.get_or_init(|| ());
        Ok(())
    }
    pub fn value(&self) -> &Value {
        &self.rendered.value
    }
    pub fn site(&self) -> &InputSite {
        &self.destination.site
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.destination.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.rendered.transaction).await?;
        self.destination.fence.ensure_fresh_on(conn).await
    }
}

impl InheritedInputRepository {
    pub async fn prepare_native_result_shorthand(
        conn: &mut PgConnection,
        view: &PreparedNativeResultView,
        directive_path: &str,
        controls: &[&PreparedInputSource],
        key: &str,
    ) -> Result<PreparedNativeRecipeDestination> {
        view.ensure_fresh_on(conn).await?;
        let execution = view_source_renderer(conn, view).await?;
        let renderer = DeclaredInputRepository::require_source(conn, execution).await?;
        if renderer.id != view.renderer {
            return Err(denied());
        }
        let (site, when, value) = Snapshots::native_shorthand_source(
            conn,
            view.renderer,
            view.snapshot,
            directive_path,
            Some(key),
            true,
        )
        .await?;
        if value.as_ref() != Some(view.value()) {
            return Err(denied());
        }
        let mut selections = vec![ReadSelection {
            context_path: "/result".into(),
            source: SelectionSource::NativeSnapshot {
                snapshot: view.snapshot,
            },
        }];
        let mut fence = view.fence.clone();
        let mut firing_control = None;
        let mut wait_reads = Vec::new();
        let mut empty_reads = Vec::new();
        let mut result_reads = view.prospective_result_reads("$native/value")?;
        for control in controls {
            for receipt in std::iter::once(&control.receipt).chain(&control.controls) {
                merge_wait_reads(&mut wait_reads, &receipt.wait_reads)?;
                merge_result_reads(&mut result_reads, &receipt.result_reads)?;
                merge_empty_reads(&mut empty_reads, &receipt.empty_reads)?;
            }
            for (receipt, site, control_fence) in
                Self::native_control_dependencies(conn, renderer.id, control).await?
            {
                if when.as_deref() == Some(site.path.as_str())
                    && site.purpose == InputSitePurpose::Condition
                    && std::iter::once(&control.receipt)
                        .chain(&control.controls)
                        .any(|source| source.id == receipt && source.value == Value::Bool(true))
                {
                    firing_control = Some(receipt);
                }
                let selection = ReadSelection {
                    context_path: "$control".into(),
                    source: SelectionSource::Control { receipt, site },
                };
                if !selections.contains(&selection) {
                    selections.push(selection);
                }
                fence = fence.intersect(&control_fence);
            }
        }
        if when.is_some() != firing_control.is_some() {
            return Err(denied());
        }
        let rendered = RenderedReceiptContext {
            result_reads,
            empty_reads,
            wait_reads,
            native: Some(NativeValueRecipe::ResultShorthand {
                snapshot: view.snapshot,
                directive_path: directive_path.into(),
                firing_control,
            }),
            transaction: view.transaction.clone(),
            renderer,
            declaration: None,
            literal: None,
            value: view.value.clone(),
            selections,
            fence,
            purpose: InputSitePurpose::Value,
        };
        let destination = Self::prepare_destination(conn, &rendered, &site, key).await?;
        let workflow = super::super::workflow::WorkflowExecutionRepository::find_by_id(
            &mut *conn,
            view.workflow(),
        )
        .await?
        .ok_or_else(denied)?;
        Ok(PreparedNativeRecipeDestination {
            rendered,
            destination,
            target_version: Value::Null,
            workflow_original: Some(serde_json::to_value(workflow)?),
            workflow_checked: std::sync::OnceLock::new(),
        })
    }
    pub async fn record_native_result_shorthand(
        conn: &mut PgConnection,
        view: &PreparedNativeResultView,
        destination: &PreparedNativeRecipeDestination,
        actual: &Value,
        key: &str,
    ) -> Result<ReceiptContext> {
        view.require_current(conn).await?;
        if !matches!(&destination.rendered.native,Some(NativeValueRecipe::ResultShorthand {snapshot,..}) if *snapshot==view.snapshot)
            || destination.rendered.renderer.id != view.renderer
            || destination.rendered.transaction != view.transaction
            || destination.workflow_checked.get().is_none()
        {
            return Err(denied());
        }
        Self::record_prepared(
            conn,
            &destination.rendered,
            &destination.destination,
            actual,
            &destination.rendered.selections,
            key,
        )
        .await
    }
    /// Actual H state only. Failure semantics do not evaluate authored output_map
    /// leaves. Success status/warnings are a separate finite output recipe.
    pub async fn prepare_native_completion_metadata(
        conn: &mut PgConnection,
        state: &PreparedNativeFailure,
        actual_output_leaves: &[&RenderedReceiptContext],
        controls: &[&PreparedInputSource],
        key: &str,
    ) -> Result<PreparedNativeCompletionMetadata> {
        if !actual_output_leaves.is_empty() {
            return Err(denied());
        }
        Snapshots::prepare_native_completion_metadata(conn, state, controls, key).await
    }
    pub async fn prepare_native_completion_destination(
        conn: &mut PgConnection,
        source: &PreparedNativeCompletionMetadata,
        key: &str,
    ) -> Result<PreparedNativeRecipeDestination> {
        source.ensure_fresh_on(conn).await?;
        let renderer = DeclaredInputRepository::require_source(conn, source.execution()).await?;
        if renderer.id != source.renderer {
            return Err(denied());
        }
        let rendered = RenderedReceiptContext {
            result_reads: Vec::new(),
            empty_reads: Vec::new(),
            wait_reads: Vec::new(),
            native: Some(NativeValueRecipe::OrdinaryFailure {
                snapshot: source.snapshot,
            }),
            transaction: source.transaction.clone(),
            renderer,
            declaration: None,
            literal: None,
            value: source.value.clone(),
            selections: vec![ReadSelection {
                context_path: "$native/value".into(),
                source: SelectionSource::NativeSnapshot {
                    snapshot: source.snapshot,
                },
            }],
            fence: source.fence.clone(),
            purpose: InputSitePurpose::Value,
        };
        let site = InputSite {
            execution: source.execution(),
            kind: InputEntityKind::ExecutionResult,
            purpose: InputSitePurpose::Value,
            path: String::new(),
        };
        let destination = Self::prepare_destination(conn, &rendered, &site, key).await?;
        let target_version = target_version(conn, site.execution).await?;
        Ok(PreparedNativeRecipeDestination {
            rendered,
            destination,
            target_version,
            workflow_original: None,
            workflow_checked: std::sync::OnceLock::new(),
        })
    }
    pub async fn record_native_completion_metadata(
        conn: &mut PgConnection,
        source: &PreparedNativeCompletionMetadata,
        destination: &PreparedNativeRecipeDestination,
        actual_failed_tasks: &[String],
        actual: &Value,
        key: &str,
    ) -> Result<ReceiptContext> {
        source.require_projected_failure(actual_failed_tasks)?;
        source.require_current(conn).await?;
        if destination
            .rendered
            .native
            .as_ref()
            .map(NativeValueRecipe::snapshot)
            != Some(source.snapshot)
            || destination.rendered.renderer.id != source.renderer
            || destination.rendered.transaction != source.transaction
            || destination.target_version != target_version(conn, source.execution()).await?
        {
            return Err(denied());
        }
        Self::record_prepared(
            conn,
            &destination.rendered,
            &destination.destination,
            actual,
            &destination.rendered.selections,
            key,
        )
        .await
    }
    pub(crate) async fn native_control_dependencies(
        conn: &mut PgConnection,
        renderer: Id,
        source: &PreparedInputSource,
    ) -> Result<Vec<(Id, InputSite, ApprovalEffectFence)>> {
        if source.renderer != renderer || source.receipt.site.purpose == InputSitePurpose::Value {
            return Err(denied());
        }
        let mut dependencies = Vec::new();
        for receipt in std::iter::once(&source.receipt).chain(&source.controls) {
            receipt.ensure_fresh_on(conn).await?;
            dependencies.push((receipt.id, receipt.site.clone(), receipt.fence.clone()));
        }
        if dependencies.len() > 256 {
            return Err(denied());
        }
        Ok(dependencies)
    }
    /// Retain the actual prepared decision, including its private issued reads.
    /// Do not replay an unpublished same-advance Condition as a public receipt.
    pub(crate) async fn native_control_contexts(
        conn: &mut PgConnection,
        renderer: Id,
        source: &PreparedInputSource,
    ) -> Result<Vec<ReceiptContext>> {
        Self::native_control_dependencies(conn, renderer, source).await?;
        Ok(std::iter::once(&source.receipt)
            .chain(&source.controls)
            .map(ReceiptContext::copy_held)
            .collect())
    }
}

pub(super) async fn target_version(conn: &mut PgConnection, execution: Id) -> Result<Value> {
    sqlx::query_scalar("SELECT jsonb_build_object('result',result,'status',status,'updated',updated) FROM execution WHERE id=$1").bind(execution).fetch_optional(conn).await?.ok_or_else(denied)
}

pub(super) async fn validate_recipe(
    conn: &mut PgConnection,
    renderer: &RuntimeAdmissionRecord,
    recipe: &NativeValueRecipe,
    site: &InputSite,
    selections: &[ReadSelection],
    plaintext: Option<(&Value, &str)>,
    pending: bool,
) -> Result<()> {
    if let NativeValueRecipe::ResultShorthand {
        snapshot,
        directive_path,
        firing_control,
    } = recipe
    {
        let (expected, when, value) = Snapshots::native_shorthand_source(
            conn,
            renderer.id,
            *snapshot,
            directive_path,
            plaintext.map(|(_, key)| key),
            pending,
        )
        .await?;
        if &expected!=site||selections.len()>256||selections.iter().filter(|selected|matches!(&selected.source,SelectionSource::NativeSnapshot {..})).count()!=1||!selections.contains(&ReadSelection {context_path:"/result".into(),source:SelectionSource::NativeSnapshot {snapshot:*snapshot}})
            ||selections.iter().any(|selection|!matches!(&selection.source,SelectionSource::NativeSnapshot {snapshot:source} if *source==*snapshot&&selection.context_path=="/result")&&!matches!(&selection.source,SelectionSource::Control {..} if selection.context_path=="$control")) {return Err(denied());}
        if let Some((actual, _)) = plaintext {
            if value.as_ref() != Some(actual) {
                return Err(denied());
            }
        }
        match (when, firing_control) {
            (None, None) => {}
            (Some(when), Some(receipt)) => {
                let control_site = selections
                    .iter()
                    .find_map(|selected| match &selected.source {
                        SelectionSource::Control {
                            receipt: actual,
                            site,
                        } if actual == receipt
                            && site.purpose == InputSitePurpose::Condition
                            && site.path == when =>
                        {
                            Some(site)
                        }
                        _ => None,
                    })
                    .ok_or_else(denied)?;
                if control_site.execution != renderer.owner_id
                    || control_site.kind != InputEntityKind::WorkflowVariables
                {
                    return Err(denied());
                }
                let (declaration,encrypted):(Value,Value)=sqlx::query_as("SELECT binding.declaration,receipt.encrypted_value FROM runtime_input_receipt receipt JOIN runtime_declared_input_binding binding ON binding.id=receipt.declaration WHERE receipt.id=$1 AND receipt.execution=$2 AND receipt.renderer_admission=$3 AND receipt.entity_kind='workflow_variables' AND receipt.destination_purpose='condition' AND receipt.destination_path=$4").bind(receipt).bind(renderer.owner_id).bind(renderer.id).bind(&control_site.path).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
                let declaration: super::super::declared_input::DeclaredInputBinding =
                    serde_json::from_value(declaration)?;
                if declaration.source_path != when {
                    return Err(denied());
                }
                DeclaredInputRepository::validate_declaration(conn, renderer, &declaration).await?;
                if let Some((_, key)) = plaintext {
                    // Compare the immutable guard bytes here. The EXISTING
                    // canonical receipt walk validates the Control's complete
                    // ancestry and fences below; do not start a second walker.
                    if crate::crypto::decrypt_json(&encrypted, key)? != Value::Bool(true) {
                        return Err(denied());
                    }
                }
            }
            _ => return Err(denied()),
        }
        return Ok(());
    }
    if site.execution != renderer.owner_id
        || site.kind != InputEntityKind::ExecutionResult
        || site.purpose != InputSitePurpose::Value
        || !site.path.is_empty()
        || selections
            != [ReadSelection {
                context_path: "$native/value".into(),
                source: SelectionSource::NativeSnapshot {
                    snapshot: recipe.snapshot(),
                },
            }]
    {
        return Err(denied());
    }
    if let NativeValueRecipe::OrdinarySuccess { snapshot } = recipe {
        return Snapshots::validate_native_success_recipe(conn, renderer.id, *snapshot, plaintext)
            .await;
    }
    if let NativeValueRecipe::WaitCompletion { snapshot } = recipe {
        return Snapshots::validate_wait_completion_recipe(conn, renderer.id, *snapshot, plaintext)
            .await;
    }
    Snapshots::validate_native_completion_recipe(
        conn,
        renderer.id,
        recipe.snapshot(),
        plaintext,
        pending,
    )
    .await
}

async fn view_source_renderer(
    conn: &mut PgConnection,
    view: &PreparedNativeResultView,
) -> Result<Id> {
    Ok(
        RuntimeAuthorityRepository::require_retained_admission(conn, view.renderer)
            .await?
            .owner_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_discriminators_do_not_accept_caller_payloads_or_expressions() {
        assert!(serde_json::from_value::<NativeValueRecipe>(json!({"recipe":"ordinary_success","snapshot":1,"succeeded":true,"output_validation_warning":{"code":"supplied","path":"/value"}})).is_err());
        assert!(serde_json::from_value::<NativeValueRecipe>(
            json!({"recipe":"ordinary_failure","snapshot":1,"value":{"error":"invented"}})
        )
        .is_err());
        assert!(serde_json::from_value::<NativeValueRecipe>(json!({"recipe":"result_shorthand","snapshot":1,"directive_path":"/workflow/tasks/0/publish/0","firing_control":null,"expression":"{{ arbitrary() }}"})).is_err());
    }
}
