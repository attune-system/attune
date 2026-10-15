use serde_json::Value;
use sqlx::{PgConnection, Postgres};

use crate::value_schema::{CompiledContract, Contract, SchemaContext, SourceIdentity};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct KeySchemaAssignment {
    pub authored_schema: Value,
    pub contract: Contract,
}

pub struct KeySchemaRepository;

impl KeySchemaRepository {
    pub async fn find<'e, E>(executor: E, key_id: i64) -> Result<Option<KeySchemaAssignment>>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let row: Option<(Value, Value)> = sqlx::query_as(
            "SELECT authored_schema, contract FROM key_schema_assignment WHERE key_id = $1",
        )
        .bind(key_id)
        .fetch_optional(executor)
        .await?;
        let Some((authored_schema, body)) = row else {
            return Ok(None);
        };
        let contract: Contract = serde_json::from_value(body)
            .map_err(|_| Error::validation("Invalid stored Key schema assignment"))?;
        validate_assignment(&authored_schema, &contract)?;
        Ok(Some(KeySchemaAssignment {
            authored_schema,
            contract,
        }))
    }

    /// The caller holds the Key row and schema-selection admission in its
    /// mutation transaction. Values are validated before encryption.
    pub async fn set(
        connection: &mut PgConnection,
        key_id: i64,
        assignment: Option<&KeySchemaAssignment>,
    ) -> Result<()> {
        let key_ref: String = sqlx::query_scalar("SELECT ref FROM key WHERE id = $1 FOR KEY SHARE")
            .bind(key_id)
            .fetch_one(&mut *connection)
            .await?;
        if let Some(assignment) = assignment {
            validate_assignment(&assignment.authored_schema, &assignment.contract)?;
            if assignment.contract.source.component_ref() != key_ref {
                return Err(Error::validation(
                    "Key schema assignment does not match its owner",
                ));
            }
            let contract = serde_json::to_value(&assignment.contract)
                .map_err(|_| Error::validation("Invalid Key schema assignment"))?;
            sqlx::query("INSERT INTO key_schema_assignment (key_id, authored_schema, contract) VALUES ($1, $2, $3) ON CONFLICT (key_id) DO UPDATE SET authored_schema = EXCLUDED.authored_schema, contract = EXCLUDED.contract, updated = NOW()")
                .bind(key_id).bind(&assignment.authored_schema).bind(contract).execute(connection).await?;
        } else {
            sqlx::query("DELETE FROM key_schema_assignment WHERE key_id = $1")
                .bind(key_id)
                .execute(connection)
                .await?;
        }
        Ok(())
    }

    pub fn prepare_value(
        assignment: &KeySchemaAssignment,
        value: &Value,
        encrypted: bool,
    ) -> Result<Value> {
        validate_assignment(&assignment.authored_schema, &assignment.contract)?;
        let contract = CompiledContract::new(&assignment.contract)
            .map_err(|error| Error::validation(format!("Invalid Key value contract: {error}")))?;
        let value = contract
            .apply_defaults(value)
            .map_err(|error| Error::validation(format!("Invalid Key value: {error}")))?;
        contract
            .validate(&value)
            .map_err(|error| Error::validation(format!("Invalid Key value: {error}")))?;
        if !encrypted
            && !contract
                .secret_paths(&value)
                .map_err(|error| {
                    Error::validation(format!("Invalid Key secret traversal: {error}"))
                })?
                .is_empty()
        {
            return Err(Error::validation(
                "Schema-secret Key values must be encrypted",
            ));
        }
        Ok(value)
    }
}

fn validate_assignment(schema: &Value, contract: &Contract) -> Result<()> {
    if !matches!(contract.source, SourceIdentity::Assigned { .. })
        || contract.context != SchemaContext::SingleValue
        || crate::value_schema::digest(schema)
            .map_err(|_| Error::validation("Invalid authored Key schema"))?
            != contract.authored_digest
    {
        return Err(Error::validation(
            "Key schema requires an authoritative single-value assignment",
        ));
    }
    CompiledContract::new(contract)
        .map_err(|error| Error::validation(format!("Invalid Key schema assignment: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_use_assigned_defaults_and_secret_fields_require_encryption() {
        let schema = serde_json::json!({"type": "object", "properties": {
            "enabled": {"type": "boolean", "default": false},
            "token": {"type": "string", "secret": true}
        }});
        let contract = crate::value_schema::compile(
            SourceIdentity::Assigned {
                component_ref: "system.fixture".into(),
                assignment_id: uuid::Uuid::new_v4(),
            },
            SchemaContext::SingleValue,
            &schema,
            &crate::value_schema::RegistryView::new(),
            crate::value_schema::Limits::default(),
        )
        .unwrap();
        let assignment = KeySchemaAssignment {
            authored_schema: schema,
            contract,
        };
        assert_eq!(
            KeySchemaRepository::prepare_value(&assignment, &serde_json::json!({}), false).unwrap(),
            serde_json::json!({"enabled": false})
        );
        let private = serde_json::json!({"token": "fixture-private-value"});
        assert!(KeySchemaRepository::prepare_value(&assignment, &private, false).is_err());
        assert_eq!(
            KeySchemaRepository::prepare_value(&assignment, &private, true).unwrap()["enabled"],
            false
        );
        assert!(KeySchemaRepository::prepare_value(
            &assignment,
            &serde_json::json!({"enabled": null}),
            true
        )
        .is_err());
    }
}
