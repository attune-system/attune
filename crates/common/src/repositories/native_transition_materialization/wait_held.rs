//! Traverse the original Sources issued specifically by Wait preparation.
//! Group/completion frames are not this source shape and must never be treated
//! as unbounded merely because the Wait adapter does not own their interface.
use super::*;

impl Sources {
    pub(in crate::repositories) fn held_wait_deadline(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        if self.iteration.is_some()
            || self.native_result.is_some()
            || self.outcome.is_some()
            || self.completion_task.is_some()
            || self.ordinary_task_path.is_some()
        {
            return Err(changed());
        }
        Ok(self
            .fence
            .approval_deadline()
            .into_iter()
            .chain(self.system.held_effect_fence().approval_deadline())
            .chain(self.fields.iter().filter_map(|field| {
                field
                    .source
                    .as_ref()
                    .and_then(PreparedInputSource::approval_deadline)
            }))
            .min())
    }

    pub(in crate::repositories) async fn ensure_wait_sources_on(
        &self,
        conn: &mut PgConnection,
    ) -> Result<()> {
        self.held_wait_deadline()?;
        let actual: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if actual != self.transaction {
            return Err(changed());
        }
        self.fence.ensure_fresh_on(conn).await?;
        self.system
            .held_effect_fence()
            .ensure_fresh_on(conn)
            .await?;
        for source in self.fields.iter().filter_map(|field| field.source.as_ref()) {
            source.ensure_fresh_on(conn).await?;
        }
        self.ensure_wait_sources()
    }

    pub(in crate::repositories) fn ensure_wait_sources(&self) -> Result<()> {
        self.held_wait_deadline()?;
        self.fence.ensure_fresh()?;
        self.system.held_effect_fence().ensure_fresh()?;
        for source in self.fields.iter().filter_map(|field| field.source.as_ref()) {
            source.ensure_fresh()?;
        }
        Ok(())
    }
}
