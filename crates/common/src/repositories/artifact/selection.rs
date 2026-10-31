//! Identity selection must precede any protected content projection.
use super::ArtifactVersionRepository;
use crate::{models::ArtifactVersion, Result};
use sqlx::{Executor, PgConnection, Postgres};

impl ArtifactVersionRepository {
    pub async fn find_selection_identity<'e, E>(
        executor: E,
        artifact: i64,
        number: Option<i32>,
    ) -> Result<Option<i64>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_scalar("SELECT id FROM artifact_version WHERE artifact = $1 AND ($2::INTEGER IS NULL OR version = $2) ORDER BY version DESC LIMIT 1")
            .bind(artifact).bind(number).fetch_optional(executor).await.map_err(Into::into)
    }

    pub async fn find_pinned_with_content_for_share(
        connection: &mut PgConnection,
        id: i64,
    ) -> Result<Option<ArtifactVersion>> {
        sqlx::query_as::<_, ArtifactVersion>(&format!(
            "SELECT {} FROM artifact_version WHERE id = $1 FOR SHARE",
            crate::models::artifact_version::SELECT_COLUMNS_WITH_CONTENT,
        ))
        .bind(id)
        .fetch_optional(connection)
        .await
        .map_err(Into::into)
    }
}
