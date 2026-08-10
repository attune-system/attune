//! Notification repository for database operations

use crate::models::{enums::NotificationState, notification::*, JsonDict};
use crate::Result;
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, List, Repository, Update};

pub struct NotificationRepository;

impl Repository for NotificationRepository {
    type Entity = Notification;
    fn table_name() -> &'static str {
        "notification"
    }
}

#[derive(Debug, Clone)]
pub struct CreateNotificationInput {
    pub channel: String,
    pub entity_type: String,
    pub entity: String,
    pub activity: String,
    pub state: NotificationState,
    pub content: Option<JsonDict>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateNotificationInput {
    pub state: Option<NotificationState>,
    pub content: Option<JsonDict>,
}

#[async_trait::async_trait]
impl FindById for NotificationRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Notification>(
            "SELECT id, channel, entity_type, entity, activity, state, content, created, updated FROM notification WHERE id = $1"
        ).bind(id).fetch_optional(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for NotificationRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Notification>(
            "SELECT id, channel, entity_type, entity, activity, state, content, created, updated FROM notification ORDER BY created DESC, id DESC LIMIT 1000"
        ).fetch_all(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for NotificationRepository {
    type CreateInput = CreateNotificationInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Notification>(
            "INSERT INTO notification (channel, entity_type, entity, activity, state, content) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, channel, entity_type, entity, activity, state, content, created, updated"
        ).bind(&input.channel).bind(&input.entity_type).bind(&input.entity).bind(&input.activity).bind(input.state).bind(&input.content).fetch_one(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for NotificationRepository {
    type UpdateInput = UpdateNotificationInput;
    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query
        let mut query = QueryBuilder::new("UPDATE notification SET ");
        let mut has_updates = false;

        if let Some(state) = input.state {
            query.push("state = ").push_bind(state);
            has_updates = true;
        }
        if let Some(content) = &input.content {
            if has_updates {
                query.push(", ");
            }
            query.push("content = ").push_bind(content);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(" RETURNING id, channel, entity_type, entity, activity, state, content, created, updated");

        query
            .build_query_as::<Notification>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for NotificationRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM notification WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl NotificationRepository {
    pub async fn find_by_state<'e, E>(
        executor: E,
        state: NotificationState,
    ) -> Result<Vec<Notification>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Notification>(
            "SELECT id, channel, entity_type, entity, activity, state, content, created, updated FROM notification WHERE state = $1 ORDER BY created DESC, id DESC"
        ).bind(state).fetch_all(executor).await.map_err(Into::into)
    }

    pub async fn find_by_channel<'e, E>(executor: E, channel: &str) -> Result<Vec<Notification>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Notification>(
            "SELECT id, channel, entity_type, entity, activity, state, content, created, updated FROM notification WHERE channel = $1 ORDER BY created DESC, id DESC"
        ).bind(channel).fetch_all(executor).await.map_err(Into::into)
    }
}
