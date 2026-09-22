use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{Executor, FromRow, PgConnection, Postgres};

use crate::{models::Id, Result};

const SELECT_COLUMNS: &str = "id, sensor, integration_identity, workload, assignment_generation, \
    pack_release, pack_release_digest, adapter_ref, provider_delivery_id, request_digest, \
    encrypted_payload, state, inquiry, rejection_code, created, updated";

#[derive(Debug, Clone, FromRow)]
pub struct InquiryCallbackDelivery {
    pub id: Id,
    pub sensor: Id,
    pub integration_identity: Id,
    pub workload: Id,
    pub assignment_generation: i64,
    pub pack_release: Id,
    pub pack_release_digest: String,
    pub adapter_ref: String,
    pub provider_delivery_id: String,
    pub request_digest: String,
    pub encrypted_payload: JsonValue,
    pub state: String,
    pub inquiry: Option<Id>,
    pub rejection_code: Option<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

pub struct CreateInquiryCallbackDelivery<'a> {
    pub sensor: Id,
    pub integration_identity: Id,
    pub workload: Id,
    pub assignment_generation: i64,
    pub pack_release: Id,
    pub pack_release_digest: &'a str,
    pub adapter_ref: &'a str,
    pub provider_delivery_id: &'a str,
    pub request_digest: &'a str,
    pub encrypted_payload: &'a JsonValue,
}

#[derive(Debug)]
pub enum InsertInquiryCallbackDelivery {
    Inserted(InquiryCallbackDelivery),
    Existing(InquiryCallbackDelivery),
    DigestConflict,
}

pub struct InquiryCallbackDeliveryRepository;

impl InquiryCallbackDeliveryRepository {
    pub async fn find_by_id<'e, E>(executor: E, id: Id) -> Result<Option<InquiryCallbackDelivery>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, InquiryCallbackDelivery>(&format!(
            "SELECT {SELECT_COLUMNS} FROM inquiry_callback_delivery WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn claim_pending_ids<'e, E>(
        executor: E,
        limit: i64,
        retry_after_seconds: i64,
    ) -> Result<Vec<Id>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_scalar(
            "WITH candidates AS ( \
                 SELECT id FROM inquiry_callback_delivery \
                 WHERE state = 'pending' AND next_attempt_at <= clock_timestamp() \
                 ORDER BY next_attempt_at, created, id LIMIT $1 \
                 FOR UPDATE SKIP LOCKED \
             ) \
             UPDATE inquiry_callback_delivery AS delivery \
             SET attempt_count = attempt_count + 1, \
                 next_attempt_at = clock_timestamp() + make_interval(secs => $2::double precision), \
                 updated = NOW() \
             FROM candidates WHERE delivery.id = candidates.id \
             RETURNING delivery.id",
        )
        .bind(limit)
        .bind(retry_after_seconds)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_sensor_adapter_provider_id<'e, E>(
        executor: E,
        sensor: Id,
        adapter_ref: &str,
        provider_delivery_id: &str,
    ) -> Result<Option<InquiryCallbackDelivery>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, InquiryCallbackDelivery>(&format!(
            "SELECT {SELECT_COLUMNS} FROM inquiry_callback_delivery \
             WHERE sensor = $1 AND adapter_ref = $2 AND provider_delivery_id = $3"
        ))
        .bind(sensor)
        .bind(adapter_ref)
        .bind(provider_delivery_id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn insert_or_load(
        conn: &mut PgConnection,
        input: CreateInquiryCallbackDelivery<'_>,
    ) -> Result<InsertInquiryCallbackDelivery> {
        let query = format!(
            "INSERT INTO inquiry_callback_delivery (sensor, integration_identity, workload, \
             assignment_generation, pack_release, pack_release_digest, adapter_ref, \
             provider_delivery_id, request_digest, encrypted_payload) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             ON CONFLICT (sensor, adapter_ref, provider_delivery_id) DO NOTHING \
             RETURNING {SELECT_COLUMNS}"
        );
        if let Some(delivery) = sqlx::query_as::<_, InquiryCallbackDelivery>(&query)
            .bind(input.sensor)
            .bind(input.integration_identity)
            .bind(input.workload)
            .bind(input.assignment_generation)
            .bind(input.pack_release)
            .bind(input.pack_release_digest)
            .bind(input.adapter_ref)
            .bind(input.provider_delivery_id)
            .bind(input.request_digest)
            .bind(input.encrypted_payload)
            .fetch_optional(&mut *conn)
            .await?
        {
            return Ok(InsertInquiryCallbackDelivery::Inserted(delivery));
        }

        let existing = sqlx::query_as::<_, InquiryCallbackDelivery>(&format!(
            "SELECT {SELECT_COLUMNS} FROM inquiry_callback_delivery \
             WHERE sensor = $1 AND adapter_ref = $2 AND provider_delivery_id = $3"
        ))
        .bind(input.sensor)
        .bind(input.adapter_ref)
        .bind(input.provider_delivery_id)
        .fetch_one(&mut *conn)
        .await?;
        if existing.request_digest != input.request_digest {
            return Ok(InsertInquiryCallbackDelivery::DigestConflict);
        }
        Ok(InsertInquiryCallbackDelivery::Existing(existing))
    }

    pub async fn find_by_id_for_update(
        conn: &mut PgConnection,
        id: Id,
    ) -> Result<Option<InquiryCallbackDelivery>> {
        sqlx::query_as::<_, InquiryCallbackDelivery>(&format!(
            "SELECT {SELECT_COLUMNS} FROM inquiry_callback_delivery WHERE id = $1 FOR UPDATE"
        ))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(Into::into)
    }

    pub async fn mark_accepted(conn: &mut PgConnection, id: Id, inquiry_id: Id) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE inquiry_callback_delivery SET state = 'accepted', inquiry = $2, \
             rejection_code = NULL, updated = NOW() WHERE id = $1 AND state = 'pending'",
        )
        .bind(id)
        .bind(inquiry_id)
        .execute(conn)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_rejected(
        conn: &mut PgConnection,
        id: Id,
        rejection_code: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE inquiry_callback_delivery SET state = 'rejected', rejection_code = $2, \
             inquiry = NULL, updated = NOW() WHERE id = $1 AND state = 'pending'",
        )
        .bind(id)
        .bind(rejection_code)
        .execute(conn)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
