//! Admitted, immutable sources for API authorization consumer tests.
use attune_common::{
    models::ExecutionStatus,
    repositories::{
        delegated_execution::DelegatedExecutionRepository, execution::ExecutionRepository, FindById,
    },
    Result,
};
use uuid::Uuid;
#[path = "delegated.rs"]
mod attempt;

/// Admit an immutable managed fixture with its owner's existing grants. This
/// bypasses caller action-execute admission only to isolate token consumers.
pub async fn create(
    pool: &sqlx::PgPool,
    mut input: attune_common::repositories::execution::CreateExecutionInput,
) -> Result<attune_common::models::Execution> {
    use attune_common::repositories::{
        executable_snapshot::ExecutableSnapshotRepository,
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    input.status = ExecutionStatus::Requested;
    let source = input.parent;
    let mut tx = pool.begin().await?;
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &input.action_ref).await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    let execution = ExecutionRepository::create_pinned(&mut *tx, input, &snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await?;
    DelegatedExecutionRepository::select(&mut tx, execution.id, source).await?;
    attempt::start(&mut tx, execution.id).await?;
    tx.commit().await?;
    Ok(execution)
}

/// Create an independent managed source for an authorization route test. No
/// permissions are assigned here: selected named sets must already be covered.
pub async fn token(ctx: &super::TestContext, owner: i64, refs: &[String]) -> super::Result<String> {
    let pack_ref = format!("credential_{}", Uuid::new_v4().simple());
    let pack = super::create_test_pack(&ctx.pool, &pack_ref).await?;
    let action_ref = format!("{pack_ref}.source");
    let action = super::create_test_action(&ctx.pool, pack.id, &pack_ref, &action_ref).await?;
    super::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &attune_common::repositories::component_lifecycle::PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await?;
    let execution = create(
        &ctx.pool,
        attune_common::repositories::execution::CreateExecutionInput {
            action: Some(action.id),
            action_ref,
            executor: Some(owner),
            permission_set_refs: refs.to_vec(),
            ..Default::default()
        },
    )
    .await?;
    execution_token(ctx, execution.id).await
}

pub async fn execution_token(ctx: &super::TestContext, execution: i64) -> super::Result<String> {
    let row = ExecutionRepository::find_by_id(&ctx.pool, execution)
        .await?
        .ok_or("fixture execution missing")?;
    let mut tx = ctx.pool.begin().await?;
    let attempt = DelegatedExecutionRepository::current_attempt(&mut tx, execution).await?;
    tx.commit().await?;
    Ok(
        attune_common::auth::jwt::generate_delegated_execution_token(
            row.executor.expect("fixture owner"),
            execution,
            &row.action_ref,
            &ctx.state.jwt_config,
            600,
            &row.permission_set_refs,
            std::slice::from_ref(&row.action_ref),
            attempt,
        )?,
    )
}
