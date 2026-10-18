//! Read-only deadline folds. Existing transaction and freshness checks remain mandatory.
use super::*;
type Deadline = Option<chrono::DateTime<chrono::Utc>>;

pub(super) fn require_deadline(date: Deadline, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
    if date.is_some_and(|date| date <= now) {
        return Err(changed());
    }
    Ok(())
}
pub(super) async fn require_deadline_on(conn: &mut PgConnection, date: Deadline) -> Result<()> {
    let now = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(conn)
        .await?;
    require_deadline(date, now)
}
impl Sources {
    pub(in crate::repositories) fn held_deadline(&self) -> Result<Deadline> {
        let mut dates = vec![
            self.fence.approval_deadline(),
            self.system.held_effect_fence().approval_deadline(),
        ];
        dates.extend(
            self.fields
                .iter()
                .filter_map(|field| field.source.as_ref())
                .map(PreparedInputSource::approval_deadline),
        );
        dates.extend(
            self.outcome
                .iter()
                .map(PreparedWorkflowOutcome::approval_deadline),
        );
        dates.extend(
            self.native_result
                .iter()
                .map(PreparedNativeResult::approval_deadline),
        );
        if let Some(group) = &self.iteration {
            dates.push(group.proof.held_deadline()?);
        }
        if let Some(empty) = &self.empty_iteration {
            dates.push(empty.empty_source()?.held_deadline()?);
            dates.push(empty.empty_outcome().ok_or_else(changed)?.held_deadline()?);
        }
        Ok(dates.into_iter().flatten().min())
    }
}
impl PreparedNativeTransitions {
    pub(in crate::repositories) fn held_deadline(&self) -> Result<Deadline> {
        let mut dates = vec![self.cache_sources.held_deadline()?];
        if let Some(empty) = &self.empty_completion {
            dates.push(empty.held_deadline());
        }
        for transition in &self.transitions {
            if let Some(condition) = &transition.condition {
                dates.push(condition.source().approval_deadline());
            }
            for publication in &transition.publications {
                for (_, proof) in &publication.leaves {
                    match proof {
                        PublicationProof::WholeResult(proof) => {
                            dates.push(proof.approval_deadline())
                        }
                        PublicationProof::Staged(proof) => dates.push(proof.approval_deadline()),
                        PublicationProof::Independent(rendered, destination) => {
                            dates.push(rendered.approval_deadline());
                            dates.push(destination.held_deadline()?);
                        }
                    }
                }
            }
        }
        if let Some(output) = &self.output {
            for (_, rendered, destination) in &output.leaves {
                dates.push(rendered.approval_deadline());
                dates.push(match destination {
                    ResultOutputDestination::Ordinary(destination) => {
                        destination.held_deadline()?
                    }
                    ResultOutputDestination::Native(destination) => destination.held_deadline()?,
                });
            }
        }
        for receipt in self
            .recorded
            .get()
            .into_iter()
            .flatten()
            .chain(self.output_receipts.get().into_iter().flatten())
        {
            dates.push(receipt.approval_deadline());
        }
        Ok(dates.into_iter().flatten().min())
    }
}
