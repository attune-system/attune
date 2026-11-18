//! Action writes own one transaction for source, runtime, schemas and references.
use serde_json::Value;
use sqlx::{Connection, PgConnection};

use super::action::{ActionRepository, CreateActionInput, UpdateActionInput};
use super::{schema_consumer, FindById, Patch};
use crate::action_cli::{schema_for_binding, ArgumentPlan};
use crate::models::{Action, ParameterDelivery, ParameterFormat};
use crate::pack_format::LaunchSpec;
use crate::value_schema::{Limits, RegistryView, SourceIdentity};
use crate::{Error, Result};

struct Definition<'a> {
    pack: i64,
    reference: &'a str,
    entrypoint: Option<&'a str>,
    launch: Option<&'a LaunchSpec>,
    runtime: Option<i64>,
    parameters: Option<&'a Value>,
    delivery: ParameterDelivery,
    format: ParameterFormat,
}

impl ActionRepository {
    pub fn create<'a, 'c, A>(
        executor: A,
        input: CreateActionInput,
    ) -> impl std::future::Future<Output = Result<Action>> + Send + 'a
    where
        A: sqlx::Acquire<'c, Database = sqlx::Postgres> + Send + 'a,
    {
        async move {
            let mut connection = executor.acquire().await?;
            let mut transaction = connection.begin().await?;
            let action = Self::create_with_registry(&mut transaction, input, None).await?;
            transaction.commit().await?;
            Ok(action)
        }
    }

    pub(crate) async fn create_with_registry(
        connection: &mut PgConnection,
        mut input: CreateActionInput,
        candidate: Option<&RegistryView>,
    ) -> Result<Action> {
        let delivery = input
            .parameter_delivery
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(Error::validation)?
            .unwrap_or_else(|| {
                input
                    .launch
                    .as_ref()
                    .map(LaunchSpec::default_parameter_delivery)
                    .unwrap_or_default()
            });
        let format: ParameterFormat = input
            .parameter_format
            .as_deref()
            .unwrap_or("json")
            .parse()
            .map_err(Error::validation)?;
        input.parameter_delivery = Some(delivery.to_string());
        input.parameter_format = Some(format.to_string());
        validate(
            connection,
            Definition {
                pack: input.pack,
                reference: &input.r#ref,
                entrypoint: input.entrypoint.as_deref(),
                launch: input.launch.as_ref(),
                runtime: input.runtime,
                parameters: input.param_schema.as_ref(),
                delivery,
                format,
            },
            candidate,
        )
        .await?;
        schema_consumer::validate_fields(
            connection,
            input.pack,
            &input.r#ref,
            &[("out_schema", input.out_schema.as_ref())],
            candidate,
        )
        .await?;
        schema_consumer::validate_output_policy(
            connection,
            input.pack,
            &input.r#ref,
            input.out_schema.as_ref(),
            &input.output_policy,
            input.output_format,
            candidate,
        )
        .await?;
        let entity = Self::create_row(&mut *connection, input).await?;
        replace_references(connection, &entity).await?;
        Ok(entity)
    }

    pub fn update<'a, 'c, A>(
        executor: A,
        id: i64,
        input: UpdateActionInput,
    ) -> impl std::future::Future<Output = Result<Action>> + Send + 'a
    where
        A: sqlx::Acquire<'c, Database = sqlx::Postgres> + Send + 'a,
    {
        async move {
            let mut connection = executor.acquire().await?;
            let mut transaction = connection.begin().await?;
            let action = Self::update_with_registry(&mut transaction, id, input, None).await?;
            transaction.commit().await?;
            Ok(action)
        }
    }

    pub(crate) async fn update_with_registry(
        connection: &mut PgConnection,
        id: i64,
        input: UpdateActionInput,
        candidate: Option<&RegistryView>,
    ) -> Result<Action> {
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        sqlx::query("SELECT id FROM action WHERE id = $1 FOR UPDATE")
            .bind(id)
            .execute(&mut *connection)
            .await?;
        let existing = Self::get_by_id(&mut *connection, id).await?;
        let launch = patched(&input.launch, existing.launch.as_ref());
        if launch.is_some() && existing.workflow_def.is_some() {
            return Err(Error::validation(
                "action.launch.invalid: Workflow actions cannot use a process launch",
            ));
        }
        let delivery = input
            .parameter_delivery
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(Error::validation)?
            .unwrap_or(existing.parameter_delivery);
        let format = input
            .parameter_format
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(Error::validation)?
            .unwrap_or(existing.parameter_format);
        validate(
            connection,
            Definition {
                pack: existing.pack,
                reference: &existing.r#ref,
                entrypoint: patched(&input.entrypoint, existing.entrypoint.as_ref())
                    .map(String::as_str),
                launch,
                runtime: input.runtime.or(existing.runtime),
                parameters: input
                    .param_schema
                    .as_ref()
                    .or(existing.param_schema.as_ref()),
                delivery,
                format,
            },
            candidate,
        )
        .await?;
        let output = input.out_schema.as_ref().or(existing.out_schema.as_ref());
        schema_consumer::validate_fields(
            connection,
            existing.pack,
            &existing.r#ref,
            &[("out_schema", output)],
            candidate,
        )
        .await?;
        schema_consumer::validate_output_policy(
            connection,
            existing.pack,
            &existing.r#ref,
            output,
            input
                .output_policy
                .as_ref()
                .unwrap_or(&existing.output_policy),
            input
                .output_format
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(Error::validation)?
                .unwrap_or(existing.output_format),
            candidate,
        )
        .await?;
        let entity = Self::update_row(&mut *connection, id, input).await?;
        replace_references(connection, &entity).await?;
        Ok(entity)
    }
}

fn patched<'a, T>(patch: &'a Option<Patch<T>>, existing: Option<&'a T>) -> Option<&'a T> {
    match patch {
        Some(Patch::Set(value)) => Some(value),
        Some(Patch::Clear) => None,
        None => existing,
    }
}

async fn replace_references(connection: &mut PgConnection, action: &Action) -> Result<()> {
    let clean = action
        .param_schema
        .as_ref()
        .map(schema_for_binding)
        .transpose()
        .map_err(|error| Error::validation(error.to_string()))?;
    schema_consumer::replace_fields(
        connection,
        action.pack,
        "action",
        &action.r#ref,
        &[
            ("param_schema", clean.as_ref()),
            ("out_schema", action.out_schema.as_ref()),
        ],
    )
    .await
}

async fn validate(
    connection: &mut PgConnection,
    definition: Definition<'_>,
    candidate: Option<&RegistryView>,
) -> Result<()> {
    let Definition {
        pack,
        reference,
        entrypoint,
        launch,
        runtime,
        parameters,
        delivery,
        format,
    } = definition;
    let Some(launch) = launch else {
        if entrypoint
            .is_none_or(|entrypoint| entrypoint.trim().is_empty() || entrypoint.len() > 1024)
            || delivery == ParameterDelivery::None
        {
            return Err(Error::validation(
                "An action requires exactly one valid execution source",
            ));
        }
        return schema_consumer::validate_fields(
            connection,
            pack,
            reference,
            &[("param_schema", parameters)],
            candidate,
        )
        .await;
    };
    launch.validate_action_source(entrypoint, delivery, format)?;
    let runtime = match runtime {
        Some(id) => super::runtime::RuntimeRepository::get_by_id(&mut *connection, id).await?,
        None => {
            return Err(Error::validation(
                "action.launch.invalid: A command launch requires an explicit native runtime",
            ))
        }
    };
    let versions = super::runtime_version::RuntimeVersionRepository::find_by_runtime(
        &mut *connection,
        runtime.id,
    )
    .await?;
    crate::action_cli::validate_native_runtime(&runtime, &versions)?;
    let empty = serde_json::json!({});
    let parameters = parameters.unwrap_or(&empty);
    let clean =
        schema_for_binding(parameters).map_err(|error| Error::validation(error.to_string()))?;
    crate::action_cli::validate_cli_input_delivery(&clean != parameters, delivery, format)?;
    let source = SourceIdentity::Authoring {
        component_ref: reference.into(),
    };
    if let Some(registry) = candidate {
        ArgumentPlan::compile(
            source,
            parameters,
            launch.action_arguments()?,
            registry,
            Limits::default(),
        )
        .map_err(|error| Error::validation(error.to_string()))?;
    } else {
        let pack = super::pack::PackRepository::get_by_id(&mut *connection, pack).await?;
        let contract = super::schema_contract::SchemaContractRepository::compile_owned_assignment(
            connection,
            source,
            crate::value_schema::SchemaContext::FlatDocument,
            &clean,
            &pack,
        )
        .await?;
        ArgumentPlan::from_bound_contract(parameters, launch.action_arguments()?, contract)
            .map_err(|error| Error::validation(error.to_string()))?;
    }
    Ok(())
}
