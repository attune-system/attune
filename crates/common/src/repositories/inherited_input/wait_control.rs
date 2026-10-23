//! Input-independent Wait handoff evidence at the child's Config CONTROL root.
//! Preparation holds the source before target locks; recording adds all actual
//! transition decisions without reopening their protected sources.
use super::super::native_input_snapshot::{
    NativeInputSnapshotRepository as Snapshots, NativeWaitOutcomeBinding,
};
use super::*;

pub(crate) struct PreparedWaitInputControl {
    rendered: RenderedReceiptContext,
    destination: PreparedReceiptDestination,
    decisions: Vec<(Option<String>, bool)>,
}

impl PreparedWaitInputControl {
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.rendered.ensure_fresh_on(conn).await?;
        self.destination.ensure_fresh_on(conn).await
    }
}

fn require_initial_selection(snapshot: Id, selections: &[ReadSelection]) -> Result<()> {
    if selections
        != [ReadSelection {
            context_path: "$control".into(),
            source: SelectionSource::NativeSnapshot { snapshot },
        }]
    {
        return Err(denied());
    }
    Ok(())
}

impl InheritedInputRepository {
    pub(crate) async fn prepare_wait_input_control(
        conn: &mut PgConnection,
        binding: NativeWaitOutcomeBinding<'_>,
        wait_reads: Vec<IssuedWaitRead>,
        child: &crate::models::Execution,
        key: &str,
    ) -> Result<PreparedWaitInputControl> {
        require_transaction(conn, binding.transaction).await?;
        binding.fence.ensure_fresh_on(conn).await?;
        let site = InputSite {
            execution: child.id,
            kind: InputEntityKind::ExecutionConfig,
            purpose: InputSitePurpose::Control,
            path: String::new(),
        };
        let canonical = Snapshots::wait_input_control_binding(
            conn,
            binding.renderer,
            binding.snapshot,
            binding.task_path,
            &site,
        )
        .await?;
        if canonical.renderer.id != binding.renderer {
            return Err(denied());
        }
        let mut rendered = RenderedReceiptContext {
            empty_reads: Vec::new(),
            result_reads: Vec::new(),
            wait_reads,
            native: Some(NativeValueRecipe::WaitInputControl {
                snapshot: binding.snapshot,
                task_path: canonical.task_path,
            }),
            transaction: binding.transaction.into(),
            renderer: canonical.renderer,
            declaration: None,
            literal: None,
            value: canonical.value,
            selections: vec![ReadSelection {
                context_path: "$control".into(),
                source: SelectionSource::NativeSnapshot {
                    snapshot: binding.snapshot,
                },
            }],
            fence: binding.fence.clone(),
            purpose: InputSitePurpose::Control,
        };
        require_initial_selection(binding.snapshot, &rendered.selections)?;
        let destination = Self::prepare_destination(conn, &rendered, &site, key).await?;
        rendered.fence = rendered.fence.intersect(&destination.fence);
        Ok(PreparedWaitInputControl {
            rendered,
            destination,
            decisions: canonical.decisions,
        })
    }

    pub(crate) async fn record_wait_input_control(
        conn: &mut PgConnection,
        prepared: &mut PreparedWaitInputControl,
        controls: &[PreparedInputSource],
        inherited: Option<&PreparedInputSource>,
        key: &str,
    ) -> Result<ReceiptContext> {
        prepared.ensure_fresh_on(conn).await?;
        let renderer = &prepared.rendered.renderer;
        let mut decisions = std::collections::BTreeMap::new();
        let mut direct_controls = Vec::new();
        let mut retained = BTreeSet::new();
        for control in controls {
            if control.renderer != renderer.id {
                return Err(denied());
            }
            for receipt in std::iter::once(&control.receipt).chain(&control.controls) {
                if receipt.site.purpose == InputSitePurpose::Condition
                    && receipt.site.kind == InputEntityKind::WorkflowVariables
                    && receipt.site.execution == renderer.owner_id
                    && prepared
                        .decisions
                        .iter()
                        .any(|(path, _)| path.as_deref() == Some(receipt.site.path.as_str()))
                {
                    if receipt.transaction != prepared.rendered.transaction {
                        return Err(denied());
                    }
                    receipt.ensure_fresh_on(conn).await?;
                    let value = receipt.value.as_bool().ok_or_else(denied)?;
                    if let Some(previous) =
                        decisions.insert(receipt.site.path.as_str(), (receipt.id, value))
                    {
                        if previous != (receipt.id, value) {
                            return Err(denied());
                        }
                    }
                    if retained.insert(receipt.id) {
                        // Its authenticated graph already retains its ancestors.
                        // Only this task's actual Conditions are direct parents;
                        // flattening source.controls would add unrelated guards.
                        direct_controls.push(PreparedInputSource {
                            renderer: renderer.id,
                            receipt: receipt.copy_held(),
                            controls: Vec::new(),
                        });
                    }
                }
            }
        }
        let mut eligible = false;
        for (path, reaches_child) in &prepared.decisions {
            let fires = match path {
                Some(path) => decisions.get(path.as_str()).ok_or_else(denied)?.1,
                None => true,
            };
            eligible |= *reaches_child && fires;
        }
        if !eligible {
            return Err(denied());
        }
        for control in &direct_controls {
            Self::attach_prepared_destination_control(
                &mut prepared.rendered,
                &mut prepared.destination,
                control,
            )?;
        }
        if let Some(source) = inherited {
            source.ensure_fresh_on(conn).await?;
            let direct = PreparedInputSource {
                renderer: source.renderer,
                receipt: source.receipt.copy_held(),
                controls: Vec::new(),
            };
            Self::attach_prepared_destination_control(
                &mut prepared.rendered,
                &mut prepared.destination,
                &direct,
            )?;
        }
        prepared.ensure_fresh_on(conn).await?;
        let Some(NativeValueRecipe::WaitInputControl {
            snapshot,
            task_path,
        }) = prepared.rendered.native.as_ref()
        else {
            return Err(denied());
        };
        Snapshots::validate_wait_input_control_recipe(
            conn,
            prepared.rendered.renderer.id,
            *snapshot,
            task_path,
            &prepared.destination.site,
            &prepared.rendered.selections,
            Some((&prepared.rendered.value, key)),
            false,
        )
        .await?;
        Self::record_prepared(
            conn,
            &prepared.rendered,
            &prepared.destination,
            &prepared.rendered.value,
            &prepared.rendered.selections,
            key,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_control_requires_exactly_its_snapshot_as_control() {
        let selection = ReadSelection {
            context_path: "$control".into(),
            source: SelectionSource::NativeSnapshot { snapshot: 1 },
        };
        assert!(require_initial_selection(1, std::slice::from_ref(&selection)).is_ok());
        assert!(require_initial_selection(2, std::slice::from_ref(&selection)).is_err());
        assert!(require_initial_selection(1, &[]).is_err());
        assert!(require_initial_selection(1, &[selection.clone(), selection.clone()]).is_err());
        let mut value_selection = selection;
        value_selection.context_path = "$native/value".into();
        assert!(require_initial_selection(1, &[value_selection]).is_err());
    }
}
