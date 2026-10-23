//! Transactional validation of mutable schema declarations. These authoring
//! contracts are never substituted for admission-time immutable bindings.

use serde_json::Value;
use sqlx::PgConnection;

use super::{pack::PackRepository, schema_contract::SchemaContractRepository, FindById};
use crate::value_schema::{compile, Limits, RegistryView, SchemaContext, SourceIdentity};
use crate::{Error, Result};

pub(crate) async fn validate_fields(
    connection: &mut PgConnection,
    pack_id: impl Into<Option<i64>> + Send,
    reference: &str,
    fields: &[(&str, Option<&Value>)],
    candidate: Option<&RegistryView>,
) -> Result<()> {
    super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
    let Some(pack_id) = pack_id.into() else {
        for (_, schema) in fields {
            if let Some(schema) = schema {
                SchemaContractRepository::materialize_unowned(
                    connection,
                    SourceIdentity::Authoring {
                        component_ref: reference.into(),
                    },
                    SchemaContext::FlatDocument,
                    schema,
                )
                .await?;
            }
        }
        return Ok(());
    };
    let pack = PackRepository::get_by_id(&mut *connection, pack_id).await?;
    if reference.split_once('.').map(|(owner, _)| owner) != Some(pack.r#ref.as_str()) {
        return Err(Error::validation(
            "Schema consumer must belong to its declaring pack",
        ));
    }
    for (_, schema) in fields {
        let Some(schema) = schema else { continue };
        let source = SourceIdentity::Authoring {
            component_ref: reference.into(),
        };
        if let Some(registry) = candidate {
            compile(
                source,
                SchemaContext::FlatDocument,
                schema,
                registry,
                Limits::default(),
            )
            .map_err(|error| {
                Error::validation(format!("Invalid candidate consumer schema: {error}"))
            })?;
        } else {
            SchemaContractRepository::compile_owned_assignment(
                &mut *connection,
                source,
                SchemaContext::FlatDocument,
                schema,
                &pack,
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) async fn replace_fields(
    connection: &mut PgConnection,
    pack: impl Into<Option<i64>> + Send,
    kind: &str,
    reference: &str,
    fields: &[(&str, Option<&Value>)],
) -> Result<()> {
    super::schema_type::SchemaTypeRepository::replace_consumer_references(
        connection,
        pack.into(),
        kind,
        reference,
        fields,
    )
    .await?;
    Ok(())
}

pub(crate) fn value_patch<'a>(
    patch: &'a Option<Value>,
    existing: Option<&'a Value>,
) -> Option<&'a Value> {
    patch.as_ref().or(existing)
}

pub(crate) fn nullable_patch<'a>(
    patch: &'a Option<super::Patch<Value>>,
    existing: Option<&'a Value>,
) -> Option<&'a Value> {
    match patch {
        Some(super::Patch::Set(schema)) => Some(schema),
        Some(super::Patch::Clear) => None,
        None => existing,
    }
}

pub(crate) fn workflow_fields<'a>(
    definition: &'a Value,
    parameters: Option<&'a Value>,
    output: Option<&'a Value>,
) -> Result<[(&'static str, Option<&'a Value>); 2]> {
    let graph_parameters = definition
        .get("parameters")
        .filter(|schema| !schema.is_null());
    let graph_output = definition.get("output").filter(|schema| !schema.is_null());
    for (metadata, graph) in [(parameters, graph_parameters), (output, graph_output)] {
        if let (Some(metadata), Some(graph)) = (metadata, graph) {
            if metadata != graph {
                return Err(Error::validation(
                    "Workflow metadata and graph declare different schemas",
                ));
            }
        }
    }
    Ok([
        ("param_schema", parameters.or(graph_parameters)),
        ("out_schema", output.or(graph_output)),
    ])
}

macro_rules! schema_consumer_repository {
    ($repository:ident, $create:ident, $update:ident, $kind:literal, $patch:ident, [$($field:ident),+] $(, graph $graph:ident)?) => {
        impl $repository {
            pub fn create<'a, 'c, A>(executor: A, input: $create) -> impl std::future::Future<Output = crate::Result<<Self as super::Repository>::Entity>> + Send + 'a
            where A: sqlx::Acquire<'c, Database = sqlx::Postgres> + Send + 'a {
                async move {
                let mut connection = executor.acquire().await?;
                let mut transaction = sqlx::Connection::begin(&mut *connection).await?;
                let entity = Self::create_with_registry(&mut transaction, input, None).await?;
                transaction.commit().await?;
                Ok(entity)
                }
            }

            pub(crate) async fn create_with_registry(
                connection: &mut sqlx::PgConnection,
                input: $create,
                candidate: Option<&crate::value_schema::RegistryView>,
            ) -> crate::Result<<Self as super::Repository>::Entity> {
                super::schema_consumer::validate_fields(connection, input.pack, &input.r#ref,
                    &[$((stringify!($field), input.$field.as_ref())),+], candidate).await?;
                $(super::schema_consumer::validate_fields(connection, input.pack, &input.r#ref,
                    &super::schema_consumer::workflow_fields(&input.$graph, input.param_schema.as_ref(), input.out_schema.as_ref())?, candidate).await?;)?
                let entity = Self::create_row(&mut *connection, input).await?;
                super::schema_consumer::replace_fields(connection, entity.pack, $kind, &entity.r#ref,
                    &[$((stringify!($field), entity.$field.as_ref())),+]).await?;
                $(super::schema_consumer::replace_fields(connection, entity.pack, $kind, &entity.r#ref,
                    &super::schema_consumer::workflow_fields(&entity.$graph, entity.param_schema.as_ref(), entity.out_schema.as_ref())?).await?;)?
                Ok(entity)
            }

            pub fn update<'a, 'c, A>(executor: A, id: i64, input: $update) -> impl std::future::Future<Output = crate::Result<<Self as super::Repository>::Entity>> + Send + 'a
            where A: sqlx::Acquire<'c, Database = sqlx::Postgres> + Send + 'a {
                async move {
                let mut connection = executor.acquire().await?;
                let mut transaction = sqlx::Connection::begin(&mut *connection).await?;
                let entity = Self::update_with_registry(&mut transaction, id, input, None).await?;
                transaction.commit().await?;
                Ok(entity)
                }
            }

            pub(crate) async fn update_with_registry(
                connection: &mut sqlx::PgConnection,
                id: i64,
                input: $update,
                candidate: Option<&crate::value_schema::RegistryView>,
            ) -> crate::Result<<Self as super::Repository>::Entity> {
                super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
                sqlx::query(&format!("SELECT id FROM {} WHERE id = $1 FOR UPDATE", <Self as super::Repository>::table_name()))
                    .bind(id).execute(&mut *connection).await?;
                let existing = Self::get_by_id(&mut *connection, id).await?;
                super::schema_consumer::validate_fields(connection, existing.pack, &existing.r#ref,
                    &[$((stringify!($field), super::schema_consumer::$patch(&input.$field, existing.$field.as_ref()))),+], candidate).await?;
                $(super::schema_consumer::validate_fields(connection, existing.pack, &existing.r#ref,
                    &super::schema_consumer::workflow_fields(input.$graph.as_ref().unwrap_or(&existing.$graph),
                        input.param_schema.as_ref().or(existing.param_schema.as_ref()), input.out_schema.as_ref().or(existing.out_schema.as_ref()))?, candidate).await?;)?
                let entity = Self::update_row(&mut *connection, id, input).await?;
                super::schema_consumer::replace_fields(connection, entity.pack, $kind, &entity.r#ref,
                    &[$((stringify!($field), entity.$field.as_ref())),+]).await?;
                $(super::schema_consumer::replace_fields(connection, entity.pack, $kind, &entity.r#ref,
                    &super::schema_consumer::workflow_fields(&entity.$graph, entity.param_schema.as_ref(), entity.out_schema.as_ref())?).await?;)?
                Ok(entity)
            }
        }
    };
}

pub(crate) use schema_consumer_repository;
