use super::FindById;
use serde_json::Value;
use sqlx::{PgConnection, Postgres};

use crate::{inquiry_schema::InquirySchemaBinding, Error, Result};

pub struct InquirySchemaRepository;

pub struct StoredInquirySchemaBinding {
    pub binding: InquirySchemaBinding,
    pub request_fingerprint: String,
}

impl InquirySchemaRepository {
    /// The caller owns the transaction. Repeated requests return their original
    /// binding before any newer type revisions are selected.
    pub async fn create_workflow(
        connection: &mut PgConnection,
        mut input: super::inquiry::CreateWorkflowInquiryInput,
        encryption_key: &str,
    ) -> Result<crate::models::inquiry::Inquiry> {
        use crate::{
            secret_values::{ENTITY_EXECUTION_CONFIG, ENTITY_INQUIRY_OPTIONS},
            value_schema::{SchemaContext, SourceIdentity},
        };
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        input.purpose = input.purpose.trim().into();
        crate::inquiry_options::validate_bound_response_options(None, &input.response_options)?;
        let fingerprint = crate::inquiry_schema::fingerprint_request(
            &serde_json::json!({
                "purpose": input.purpose, "prompt": input.prompt, "schema": input.response_schema, "options": input.response_options,
                "assigned_to": input.assigned_to, "timeout_seconds": input.timeout_seconds,
            }),
            encryption_key,
        )?;
        if let Some(existing) = super::inquiry::InquiryRepository::find_workflow_by_creator_purpose(
            &mut *connection,
            input.created_by_execution,
            &input.purpose,
        )
        .await?
        {
            return Self::check_repeated(&mut *connection, existing, &fingerprint).await;
        }
        let creator = super::execution::ExecutionRepository::find_by_id(
            &mut *connection,
            input.created_by_execution,
        )
        .await?
        .ok_or_else(|| Error::validation("Inquiry creator execution is unavailable"))?;
        let unrestricted = Value::Bool(true);
        let context = if input.response_schema.is_some() {
            SchemaContext::FlatDocument
        } else {
            SchemaContext::SingleValue
        };
        let schema = input.response_schema.as_ref().unwrap_or(&unrestricted);
        let contract = if let Some(snapshot) = &creator.executable_snapshot {
            super::schema_contract::SchemaContractRepository::materialize_dynamic(
                &mut *connection,
                snapshot,
                context,
                schema,
            )
            .await?
        } else {
            // Inline runtime-authored schemas do not need an executable release
            // to select dependencies. Named schemas never use this path.
            crate::value_schema::compile(
                SourceIdentity::Assigned {
                    component_ref: creator.action_ref.clone(),
                    assignment_id: uuid::Uuid::new_v4(),
                },
                context,
                schema,
                &crate::value_schema::RegistryView::new(),
                crate::value_schema::Limits::default(),
            )
            .map_err(|error| {
                Error::validation(format!(
                    "Inquiry schema requires a valid creator source: {error}"
                ))
            })?
        };
        let binding = InquirySchemaBinding {
            response_schema: input.response_schema.clone(),
            contract,
        };
        let input_secrets =
            super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *connection,
                ENTITY_EXECUTION_CONFIG,
                creator.id,
            )
            .await?;
        let origins = crate::secret_provenance::SecretProvenance {
            origins: input_secrets
                .iter()
                .map(|secret| crate::secret_provenance::SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: creator.id,
                    path: secret.json_path.clone(),
                })
                .collect(),
            templates: Vec::new(),
        };
        let (options, inputs) = binding.prepare_options(&input.response_options, &origins)?;
        input.response_options = options;
        let prepared = crate::secret_values::prepare_secret_values(inputs, encryption_key)?;
        let (inquiry, created) =
            super::inquiry::InquiryRepository::create_prepared_workflow_inquiry(
                &mut *connection,
                input,
            )
            .await?;
        if !created {
            return Self::check_repeated(connection, inquiry, &fingerprint).await;
        }
        Self::attach(&mut *connection, inquiry.id, &binding, &fingerprint).await?;
        super::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(
            connection,
            ENTITY_INQUIRY_OPTIONS,
            inquiry.id,
            &prepared,
        )
        .await?;
        Ok(inquiry)
    }

    async fn check_repeated(
        connection: &mut PgConnection,
        inquiry: crate::models::inquiry::Inquiry,
        fingerprint: &str,
    ) -> Result<crate::models::inquiry::Inquiry> {
        let stored = Self::find(connection, inquiry.id)
            .await?
            .ok_or_else(|| Error::validation("Inquiry is missing its original schema binding"))?;
        if stored.request_fingerprint != fingerprint {
            return Err(Error::AlreadyExists {
                entity: "inquiry".into(),
                field: "workflow task purpose".into(),
                value: "request differs from its original admission".into(),
            });
        }
        Ok(inquiry)
    }
    pub async fn find<'e, E>(executor: E, id: i64) -> Result<Option<StoredInquirySchemaBinding>>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let row: Option<(Value, String, String)> = sqlx::query_as("SELECT binding, binding_digest, request_fingerprint FROM inquiry_schema_binding WHERE entity_id = $1")
            .bind(id).fetch_optional(executor).await?;
        let Some((body, stored_digest, request_fingerprint)) = row else {
            return Ok(None);
        };
        Self::decode_binding(body, &stored_digest, request_fingerprint).map(Some)
    }

    pub async fn find_many<'e, E>(
        executor: E,
        ids: &[i64],
    ) -> Result<Vec<(i64, StoredInquirySchemaBinding)>>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let rows: Vec<(i64, Value, String, String)> = sqlx::query_as("SELECT entity_id, binding, binding_digest, request_fingerprint FROM inquiry_schema_binding WHERE entity_id = ANY($1)")
            .bind(ids).fetch_all(executor).await?;
        rows.into_iter()
            .map(|(id, body, hash, fingerprint)| {
                Self::decode_binding(body, &hash, fingerprint).map(|binding| (id, binding))
            })
            .collect()
    }

    fn decode_binding(
        body: Value,
        stored_digest: &str,
        request_fingerprint: String,
    ) -> Result<StoredInquirySchemaBinding> {
        let digest = crate::value_schema::digest(&body)
            .map_err(|_| Error::validation("Invalid stored inquiry binding digest"))?;
        if digest != stored_digest {
            return Err(Error::validation(
                "Stored inquiry schema binding digest does not match",
            ));
        }
        let binding: InquirySchemaBinding = serde_json::from_value(body)
            .map_err(|_| Error::validation("Invalid stored inquiry schema binding"))?;
        binding.validate()?;
        Ok(StoredInquirySchemaBinding {
            binding,
            request_fingerprint,
        })
    }

    pub async fn attach(
        connection: &mut PgConnection,
        id: i64,
        binding: &InquirySchemaBinding,
        request_fingerprint: &str,
    ) -> Result<()> {
        binding.validate()?;
        let schema: Option<Value> =
            sqlx::query_scalar("SELECT response_schema FROM inquiry WHERE id = $1 FOR KEY SHARE")
                .bind(id)
                .fetch_one(&mut *connection)
                .await?;
        if schema != binding.response_schema {
            return Err(Error::validation(
                "Inquiry response binding differs from its authored schema",
            ));
        }
        let body = serde_json::to_value(binding)
            .map_err(|_| Error::validation("Invalid inquiry binding body"))?;
        let digest = crate::value_schema::digest(&body)
            .map_err(|_| Error::validation("Invalid inquiry binding digest"))?;
        let inserted = sqlx::query("INSERT INTO inquiry_schema_binding (entity_id, binding, binding_digest, request_fingerprint) VALUES ($1, $2, $3, $4) ON CONFLICT (entity_id) DO NOTHING")
            .bind(id).bind(&body).bind(&digest).bind(request_fingerprint).execute(&mut *connection).await?;
        if inserted.rows_affected() == 0 {
            let stored = Self::find(&mut *connection, id)
                .await?
                .ok_or_else(|| Error::validation("Inquiry binding attachment failed"))?;
            if stored.binding != *binding || stored.request_fingerprint != request_fingerprint {
                return Err(Error::validation(
                    "Inquiry schema bindings cannot be replaced",
                ));
            }
        }
        Ok(())
    }
}
