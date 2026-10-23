//! Workflow management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use validator::Validate;

use attune_common::repositories::{
    action::{ActionRepository, CreateActionInput, UpdateActionInput},
    pack::PackRepository,
    workflow::{
        CreateWorkflowDefinitionInput, UpdateWorkflowDefinitionInput, WorkflowDefinitionRepository,
        WorkflowSearchFilters,
    },
    FindByRef, Patch,
};
use attune_common::{
    action_visibility::{collect_workflow_action_refs, ensure_action_reference_allowed},
    models::ActionReferenceVisibility,
    rbac::{Action, AuthorizationContext, Resource},
    schema::RefValidator,
    workflow::parser::WorkflowDefinition as ParsedWorkflowDefinition,
};

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::AuthorizationService,
    dto::{
        common::{PaginatedResponse, PaginationParams},
        workflow::{
            CreateWorkflowRequest, SaveWorkflowFileRequest, UpdateWorkflowRequest,
            WorkflowResponse, WorkflowSearchParams, WorkflowSummary,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

struct CompanionActionUpdate<'a> {
    label: Option<&'a str>,
    description: Option<&'a str>,
    param_schema: Option<&'a serde_json::Value>,
    out_schema: Option<&'a serde_json::Value>,
    reference_visibility: Option<ActionReferenceVisibility>,
    reference_allowed_pack_refs: Option<&'a [String]>,
}

// Only builder presentation/identity metadata is ignored. In particular, do not
// recursively strip similarly named keys from parameters, variables or payloads.
// Comparing only a typed reserialization would discard unknown authored fields.
// Normalize known parser defaults in place instead, retaining opaque fields.
fn workflow_behavior_definition(
    definition: &serde_json::Value,
    input: Option<&serde_json::Value>,
    output: Option<&serde_json::Value>,
) -> serde_json::Value {
    fn optional(object: &mut serde_json::Map<String, serde_json::Value>, fields: &[&str]) {
        for field in fields {
            if object.get(*field).is_some_and(serde_json::Value::is_null) {
                object.remove(*field);
            }
        }
    }
    fn tasks(collection: &mut serde_json::Value) {
        fn task(value: &mut serde_json::Value) {
            if let Some(value) = value.as_object_mut() {
                value.remove("__chart_meta__");
                optional(
                    value,
                    &[
                        "action",
                        "action_ref",
                        "permission_set_refs",
                        "permission_set_ref",
                        "trace_tag_template",
                        "worker_selector",
                        "worker_tolerations",
                        "worker_affinity",
                        "when",
                        "delay",
                        "with_items",
                        "iterate_cache",
                        "batch_size",
                        "concurrency",
                        "retry",
                        "timeout",
                        "join",
                        "wait_for",
                        "on_success",
                        "on_failure",
                        "on_complete",
                        "on_timeout",
                        "decision",
                        "publish",
                        "tasks",
                    ],
                );
                // The loader serializes the typed parser's defaults. File PUT
                // also accepts their authored omission, without changing task
                // behavior. Keep every non-default value and opaque payload.
                if value.get("type").and_then(serde_json::Value::as_str) == Some("action") {
                    value.remove("type");
                }
                for field in ["input", "params"] {
                    if value
                        .get(field)
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(serde_json::Map::is_empty)
                    {
                        value.remove(field);
                    }
                }
                for field in ["next", "decision", "publish"] {
                    if value
                        .get(field)
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(Vec::is_empty)
                    {
                        value.remove(field);
                    }
                }
                if let Some(serde_json::Value::Object(retry)) = value.get_mut("retry") {
                    optional(retry, &["max_delay", "on_error"]);
                    if retry.get("backoff").and_then(serde_json::Value::as_str) == Some("constant")
                    {
                        retry.remove("backoff");
                    }
                }
                if let Some(serde_json::Value::Object(cache)) = value.get_mut("iterate_cache") {
                    optional(
                        cache,
                        &[
                            "owner_type",
                            "owner_ref",
                            "generation",
                            "page_size",
                            "require_fresh",
                        ],
                    );
                    for (field, default) in [
                        ("generation", serde_json::json!("active")),
                        ("page_size", serde_json::json!(100)),
                        ("require_fresh", serde_json::json!(false)),
                    ] {
                        if cache.get(field) == Some(&default) {
                            cache.remove(field);
                        }
                    }
                }
                if let Some(serde_json::Value::Array(transitions)) = value.get_mut("next") {
                    for transition in transitions {
                        if let Some(transition) = transition.as_object_mut() {
                            transition.remove("__chart_meta__");
                            optional(transition, &["when", "publish", "do"]);
                            for field in ["publish", "do"] {
                                if transition
                                    .get(field)
                                    .and_then(serde_json::Value::as_array)
                                    .is_some_and(Vec::is_empty)
                                {
                                    transition.remove(field);
                                }
                            }
                        }
                    }
                }
                if let Some(children) = value.get_mut("tasks") {
                    tasks(children);
                }
            }
        }
        match collection {
            serde_json::Value::Array(values) => values.iter_mut().for_each(task),
            serde_json::Value::Object(values) => values.values_mut().for_each(task),
            _ => {}
        }
    }
    let mut definition = definition.clone();
    if let Some(root) = definition.as_object_mut() {
        for field in ["ref", "label", "description", "tags", "version"] {
            root.remove(field);
        }
        optional(root, &["parameters", "output", "vars", "output_map"]);
        if root
            .get("parameters")
            .is_some_and(|schema| Some(schema) == input)
        {
            root.remove("parameters");
        }
        if root
            .get("output")
            .is_some_and(|schema| Some(schema) == output)
        {
            root.remove("output");
        }
        for field in ["vars", "output_map"] {
            if root
                .get(field)
                .and_then(serde_json::Value::as_object)
                .is_some_and(serde_json::Map::is_empty)
            {
                root.remove(field);
            }
        }
        if root
            .get("cancellation_policy")
            .and_then(serde_json::Value::as_str)
            == Some("allow_finish")
        {
            root.remove("cancellation_policy");
        }
        if let Some(collection) = root.get_mut("tasks") {
            tasks(collection);
        }
    }
    definition
}

fn workflow_editor(user: &AuthenticatedUser) -> ApiResult<i64> {
    user.identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".into()))
}

async fn authorize_workflow_update(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    workflow: &attune_common::models::workflow::WorkflowDefinition,
) -> ApiResult<Option<(i64, i64, bool, Option<i64>)>> {
    let companion = ActionRepository::find_by_workflow_def(&state.db, workflow.id).await?;
    if companion.as_ref().is_some_and(|action| {
        action.pack != workflow.pack
            || action.r#ref != workflow.r#ref
            || action.pack_ref != workflow.pack_ref
    }) {
        return Err(ApiError::Conflict(
            "Workflow companion action has a different structural owner or reference".into(),
        ));
    }
    let mut context = AuthorizationContext::new(workflow_editor(user)?);
    context.target_id = companion.as_ref().map(|action| action.id);
    context.target_ref = Some(workflow.r#ref.clone());
    context.pack_ref = Some(workflow.pack_ref.clone());
    let managing_pack = match &companion {
        Some(action) => {
            attune_common::repositories::iam_ownership::IamOwnershipRepository::component_pack(
                &state.db,
                attune_common::repositories::runtime_target::ComponentTarget::Action,
                action.id,
            )
            .await?
        }
        None => None,
    };
    context.managing_pack_id = managing_pack;
    state
        .authorization_service()
        .authorize(
            user,
            crate::authz::AuthorizationCheck {
                resource: Resource::Actions,
                action: Action::Update,
                context,
            },
        )
        .await?;
    Ok(companion.map(|action| (action.id, action.pack, action.is_adhoc, managing_pack)))
}

async fn check_authorized_companion(
    connection: &mut sqlx::PgConnection,
    workflow: i64,
    authorized: Option<(i64, i64, bool, Option<i64>)>,
) -> ApiResult<()> {
    let current = ActionRepository::find_by_workflow_def(&mut *connection, workflow).await?;
    let managing_pack = match &current {
        Some(action) => {
            attune_common::repositories::iam_ownership::IamOwnershipRepository::component_pack(
                &mut *connection,
                attune_common::repositories::runtime_target::ComponentTarget::Action,
                action.id,
            )
            .await?
        }
        None => None,
    };
    let current = current.map(|action| (action.id, action.pack, action.is_adhoc, managing_pack));
    if current != authorized {
        return Err(ApiError::Conflict(
            "Workflow action ownership changed during authorization".into(),
        ));
    }
    Ok(())
}

fn can_read_workflow_with_grants(
    grants: &[attune_common::rbac::Grant],
    identity_id: i64,
    workflow: &attune_common::models::workflow::WorkflowDefinition,
    managing_pack_id: Option<i64>,
    attributes: &std::collections::HashMap<String, serde_json::Value>,
) -> bool {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.managing_pack_id = managing_pack_id;
    ctx.identity_attributes = attributes.clone();
    ctx.target_id = Some(workflow.id);
    ctx.target_ref = Some(workflow.r#ref.clone());
    ctx.pack_ref = Some(workflow.pack_ref.clone());
    AuthorizationService::is_allowed(grants, Resource::Actions, Action::Read, &ctx)
}

async fn filter_api_visible_workflows(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    workflows: Vec<attune_common::models::workflow::WorkflowDefinition>,
) -> ApiResult<Vec<attune_common::models::workflow::WorkflowDefinition>> {
    let authz = state.authorization_service();
    let snapshot = authz.load_snapshot(user).await?;
    let grants = snapshot
        .as_ref()
        .map(|snapshot| snapshot.grants.as_slice())
        .unwrap_or_default();
    let attributes = snapshot
        .as_ref()
        .map(|snapshot| snapshot.identity_attributes.clone())
        .unwrap_or_default();
    let owners = if grants.iter().any(|grant| {
        grant.resource == Resource::Actions
            && grant
                .constraints
                .as_ref()
                .is_some_and(|scope| scope.managing_pack_id.is_some())
    }) {
        attune_common::repositories::iam_ownership::IamOwnershipRepository::workflow_packs(
            &state.db,
            &workflows
                .iter()
                .map(|workflow| workflow.id)
                .collect::<Vec<_>>(),
        )
        .await?
    } else {
        Default::default()
    };
    let identity_id = user.identity_id().ok();

    let mut visible = Vec::with_capacity(workflows.len());
    for workflow in workflows {
        let companion_action =
            ActionRepository::find_by_workflow_def(&state.db, workflow.id).await?;
        // Definitions created before companion actions existed have no visibility
        // metadata, so retain their historical public behavior.
        let is_public = companion_action
            .as_ref()
            .is_none_or(|action| action.reference_visibility == ActionReferenceVisibility::Public);
        if is_public
            || identity_id.is_some_and(|id| {
                can_read_workflow_with_grants(
                    grants,
                    id,
                    &workflow,
                    owners.get(&workflow.id).copied(),
                    &attributes,
                )
            })
        {
            visible.push(workflow);
        }
    }

    Ok(visible)
}

async fn can_access_workflow_api(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    workflow: &attune_common::models::workflow::WorkflowDefinition,
) -> ApiResult<bool> {
    Ok(
        filter_api_visible_workflows(state, user, vec![workflow.clone()])
            .await?
            .into_iter()
            .next()
            .is_some(),
    )
}

async fn visible_workflow_page(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    mut filters: WorkflowSearchFilters,
    pagination: &PaginationParams,
) -> ApiResult<PaginatedResponse<WorkflowSummary>> {
    filters.limit = 1;
    filters.offset = 0;
    let initial = WorkflowDefinitionRepository::list_search(&state.db, &filters).await?;

    let all_rows = if initial.total == 0 {
        Vec::new()
    } else {
        filters.limit = u32::try_from(initial.total).unwrap_or(u32::MAX);
        filters.offset = 0;
        WorkflowDefinitionRepository::list_search(&state.db, &filters)
            .await?
            .rows
    };

    let visible = filter_api_visible_workflows(state, user, all_rows).await?;
    let total = visible.len() as u64;
    let rows = visible
        .into_iter()
        .skip(pagination.offset() as usize)
        .take(pagination.limit() as usize)
        .map(WorkflowSummary::from)
        .collect();

    Ok(PaginatedResponse::new(rows, pagination, total))
}

/// List all workflows with pagination and filtering
#[utoipa::path(
    get,
    path = "/api/v1/workflows",
    tag = "workflows",
    params(PaginationParams, WorkflowSearchParams),
    responses(
        (status = 200, description = "List of workflows", body = PaginatedResponse<WorkflowSummary>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_workflows(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(pagination): Query<PaginationParams>,
    Query(search_params): Query<WorkflowSearchParams>,
) -> ApiResult<impl IntoResponse> {
    // Validate search params
    search_params.validate()?;

    // Parse comma-separated tags into a Vec if provided
    let tags = search_params.tags.as_ref().map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>()
    });

    let filters = WorkflowSearchFilters {
        pack: None,
        pack_ref: search_params.pack_ref.clone(),
        tags,
        search: search_params.search.clone(),
        limit: 0,
        offset: 0,
    };

    let response = visible_workflow_page(&state, &user, filters, &pagination).await?;

    Ok((StatusCode::OK, Json(response)))
}

/// List workflows by pack reference
#[utoipa::path(
    get,
    path = "/api/v1/packs/{pack_ref}/workflows",
    tag = "workflows",
    params(
        ("pack_ref" = String, Path, description = "Pack reference identifier"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of workflows for pack", body = PaginatedResponse<WorkflowSummary>),
        (status = 404, description = "Pack not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_workflows_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Verify pack exists
    let _pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let filters = WorkflowSearchFilters {
        pack: None,
        pack_ref: Some(pack_ref),
        tags: None,
        search: None,
        limit: 0,
        offset: 0,
    };

    let response = visible_workflow_page(&state, &user, filters, &pagination).await?;

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single workflow by reference
#[utoipa::path(
    get,
    path = "/api/v1/workflows/{ref}",
    tag = "workflows",
    params(
        ("ref" = String, Path, description = "Workflow reference identifier")
    ),
    responses(
        (status = 200, description = "Workflow details", body = inline(ApiResponse<WorkflowResponse>)),
        (status = 404, description = "Workflow not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_workflow(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(workflow_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let workflow = WorkflowDefinitionRepository::find_by_ref(&state.db, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Workflow '{}' not found", workflow_ref)))?;
    if !can_access_workflow_api(&state, &user, &workflow).await? {
        return Err(ApiError::NotFound(format!(
            "Workflow '{}' not found",
            workflow_ref
        )));
    }

    let response = ApiResponse::new(WorkflowResponse::from(workflow));

    Ok((StatusCode::OK, Json(response)))
}

/// Create a new workflow
#[utoipa::path(
    post,
    path = "/api/v1/workflows",
    tag = "workflows",
    request_body = CreateWorkflowRequest,
    responses(
        (status = 201, description = "Workflow created successfully", body = inline(ApiResponse<WorkflowResponse>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Pack not found"),
        (status = 409, description = "Workflow with same ref already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_workflow(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Json(request): Json<CreateWorkflowRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if workflow with same ref already exists
    if WorkflowDefinitionRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Workflow with ref '{}' already exists",
            request.r#ref
        )));
    }

    // Verify pack exists and get its ID
    let pack = PackRepository::find_by_ref(&state.db, &request.pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", request.pack_ref)))?;
    validate_workflow_action_references(&state, &pack.r#ref, &request.r#ref, &request.definition)
        .await?;

    // Create workflow input
    let workflow_input = CreateWorkflowDefinitionInput {
        r#ref: request.r#ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: request.label.clone(),
        description: request.description.clone(),
        version: request.version.clone(),
        param_schema: request.param_schema.clone(),
        out_schema: request.out_schema.clone(),
        definition: request.definition,
        tags: request.tags.clone().unwrap_or_default(),
    };

    let workflow = WorkflowDefinitionRepository::create(&state.db, workflow_input).await?;

    // Create a companion action record so the workflow appears in action lists
    let mut connection = state.db.acquire().await?;
    create_companion_action(
        &mut connection,
        &workflow.r#ref,
        pack.id,
        &pack.r#ref,
        &request.label,
        request.description.as_deref(),
        "workflow",
        request.param_schema.as_ref(),
        request.out_schema.as_ref(),
        ActionReferenceVisibility::Public,
        &[],
        workflow.id,
    )
    .await?;

    let response = ApiResponse::with_message(
        WorkflowResponse::from(workflow),
        "Workflow created successfully",
    );

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing workflow
#[utoipa::path(
    put,
    path = "/api/v1/workflows/{ref}",
    tag = "workflows",
    params(
        ("ref" = String, Path, description = "Workflow reference identifier")
    ),
    request_body = UpdateWorkflowRequest,
    responses(
        (status = 200, description = "Workflow updated successfully", body = inline(ApiResponse<WorkflowResponse>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Workflow not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_workflow(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(workflow_ref): Path<String>,
    Json(request): Json<UpdateWorkflowRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if workflow exists
    let existing_workflow = WorkflowDefinitionRepository::find_by_ref(&state.db, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Workflow '{}' not found", workflow_ref)))?;
    let authorized_companion = authorize_workflow_update(&state, &user, &existing_workflow).await?;
    if let Some(definition) = &request.definition {
        validate_workflow_action_references(
            &state,
            &existing_workflow.pack_ref,
            &existing_workflow.r#ref,
            definition,
        )
        .await?;
    }

    let metadata_changed = request.definition.is_some()
        || request.param_schema.is_some()
        || request.out_schema.is_some();
    let mut tx = state.db.begin().await?;
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await?;
    let current = WorkflowDefinitionRepository::find_by_ref(&mut *tx, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::Conflict("Workflow changed during authorization".into()))?;
    if current.id != existing_workflow.id
        || current.pack != existing_workflow.pack
        || current.updated != existing_workflow.updated
    {
        return Err(ApiError::Conflict(
            "Workflow ownership changed during authorization".into(),
        ));
    }
    check_authorized_companion(&mut tx, existing_workflow.id, authorized_companion).await?;
    // Create update input
    let update_input = UpdateWorkflowDefinitionInput {
        label: request.label.clone(),
        description: request.description.clone(),
        version: request.version.clone(),
        param_schema: request.param_schema.clone(),
        out_schema: request.out_schema.clone(),
        definition: request.definition,
        tags: request.tags,
    };

    let workflow =
        WorkflowDefinitionRepository::update(&mut *tx, existing_workflow.id, update_input).await?;

    // Update the companion action record if it exists
    update_companion_action(
        &mut tx,
        existing_workflow.id,
        CompanionActionUpdate {
            label: request.label.as_deref(),
            description: request.description.as_deref(),
            param_schema: request.param_schema.as_ref(),
            out_schema: request.out_schema.as_ref(),
            reference_visibility: None,
            reference_allowed_pack_refs: None,
        },
    )
    .await?;
    if metadata_changed {
        if let Some(action) =
            ActionRepository::find_by_workflow_def(&mut *tx, existing_workflow.id).await?
        {
            attune_common::repositories::managed_component_readmission::ManagedComponentReadmission::workflow(&mut tx, action.id, workflow_editor(&user)?).await?;
        }
    }
    tx.commit().await?;

    let response = ApiResponse::with_message(
        WorkflowResponse::from(workflow),
        "Workflow updated successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Delete a workflow
#[utoipa::path(
    delete,
    path = "/api/v1/workflows/{ref}",
    tag = "workflows",
    params(
        ("ref" = String, Path, description = "Workflow reference identifier")
    ),
    responses(
        (status = 200, description = "Workflow deleted successfully", body = SuccessResponse),
        (status = 404, description = "Workflow not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(workflow_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if workflow exists
    let workflow = WorkflowDefinitionRepository::find_by_ref(&state.db, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Workflow '{}' not found", workflow_ref)))?;

    // Delete the workflow (companion action is cascade-deleted via FK on action.workflow_def)
    let deleted = WorkflowDefinitionRepository::delete(&state.db, workflow.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Workflow '{}' not found",
            workflow_ref
        )));
    }

    let response =
        SuccessResponse::new(format!("Workflow '{}' deleted successfully", workflow_ref));

    Ok((StatusCode::OK, Json(response)))
}

/// Save a workflow file to disk and sync it to the database
///
/// Writes a `{name}.workflow.yaml` file to `{packs_base_dir}/{pack_ref}/actions/workflows/`
/// and creates or updates the corresponding workflow_definition record in the database.
/// Also creates a companion action record so the workflow appears in action lists and palettes.
#[utoipa::path(
    post,
    path = "/api/v1/packs/{pack_ref}/workflow-files",
    tag = "workflows",
    params(
        ("pack_ref" = String, Path, description = "Pack reference identifier")
    ),
    request_body = SaveWorkflowFileRequest,
    responses(
        (status = 201, description = "Workflow file saved and synced", body = inline(ApiResponse<WorkflowResponse>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Pack not found"),
        (status = 409, description = "Workflow with same ref already exists"),
        (status = 500, description = "Failed to write workflow file")
    ),
    security(("bearer_auth" = []))
)]
pub async fn save_workflow_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
    Json(request): Json<SaveWorkflowFileRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    // Verify pack exists
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let workflow_ref = format!("{}.{}", pack_ref, request.name);
    validate_reference_visibility_request(&request)?;
    validate_workflow_action_references(&state, &pack.r#ref, &workflow_ref, &request.definition)
        .await?;

    // Check if workflow already exists
    if WorkflowDefinitionRepository::find_by_ref(&state.db, &workflow_ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Workflow with ref '{}' already exists",
            workflow_ref
        )));
    }

    // Write YAML file to disk
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);
    write_workflow_yaml(&packs_base_dir, &pack_ref, &request).await?;

    // Create workflow in database
    let definition_json = serde_json::to_value(&request.definition).map_err(|e| {
        ApiError::BadRequest(format!("Failed to serialize workflow definition: {}", e))
    })?;

    let workflow_input = CreateWorkflowDefinitionInput {
        r#ref: workflow_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: request.label.clone(),
        description: request.description.clone(),
        version: request.version.clone(),
        param_schema: request.param_schema.clone(),
        out_schema: request.out_schema.clone(),
        definition: definition_json,
        tags: request.tags.clone().unwrap_or_default(),
    };

    let workflow = WorkflowDefinitionRepository::create(&state.db, workflow_input).await?;

    // Create a companion action record so the workflow appears in action lists and palettes
    let entrypoint = format!("workflows/{}.workflow.yaml", request.name);
    let mut connection = state.db.acquire().await?;
    create_companion_action(
        &mut connection,
        &workflow_ref,
        pack.id,
        &pack.r#ref,
        &request.label,
        request.description.as_deref(),
        &entrypoint,
        request.param_schema.as_ref(),
        request.out_schema.as_ref(),
        request
            .reference_visibility
            .unwrap_or(ActionReferenceVisibility::Public),
        &request.reference_allowed_pack_refs,
        workflow.id,
    )
    .await?;

    let response = ApiResponse::with_message(
        WorkflowResponse::from(workflow),
        "Workflow file saved and synced successfully",
    );

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update a workflow file on disk and sync changes to the database
#[utoipa::path(
    put,
    path = "/api/v1/workflows/{ref}/file",
    tag = "workflows",
    params(
        ("ref" = String, Path, description = "Workflow reference identifier")
    ),
    request_body = SaveWorkflowFileRequest,
    responses(
        (status = 200, description = "Workflow file updated and synced", body = inline(ApiResponse<WorkflowResponse>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Workflow not found"),
        (status = 500, description = "Failed to write workflow file")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_workflow_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(workflow_ref): Path<String>,
    Json(request): Json<SaveWorkflowFileRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    // Check if workflow exists
    let existing_workflow = WorkflowDefinitionRepository::find_by_ref(&state.db, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Workflow '{}' not found", workflow_ref)))?;
    let authorized_companion = authorize_workflow_update(&state, &user, &existing_workflow).await?;
    if request.pack_ref != existing_workflow.pack_ref
        || format!("{}.{}", request.pack_ref, request.name) != workflow_ref
    {
        return Err(ApiError::BadRequest(
            "Workflow file edits cannot change component ownership or reference".into(),
        ));
    }

    // Verify pack exists
    let pack = PackRepository::find_by_ref(&state.db, &request.pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", request.pack_ref)))?;
    validate_reference_visibility_request(&request)?;
    validate_workflow_action_references(&state, &pack.r#ref, &workflow_ref, &request.definition)
        .await?;

    // Publish files only after database admission commits. Never write into an
    // immutable release content directory.
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);
    let file_request = request.clone();
    let mut tx = state.db.begin().await?;
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await?;
    let current = WorkflowDefinitionRepository::find_by_ref(&mut *tx, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::Conflict("Workflow changed during authorization".into()))?;
    if current.id != existing_workflow.id
        || current.pack != existing_workflow.pack
        || current.updated != existing_workflow.updated
    {
        return Err(ApiError::Conflict(
            "Workflow ownership changed during authorization".into(),
        ));
    }
    check_authorized_companion(&mut tx, existing_workflow.id, authorized_companion).await?;

    let previous_companion =
        ActionRepository::find_by_workflow_def(&mut *tx, existing_workflow.id).await?;

    // Update workflow in database
    let definition_json = serde_json::to_value(&request.definition).map_err(|e| {
        ApiError::BadRequest(format!("Failed to serialize workflow definition: {}", e))
    })?;

    let update_input = UpdateWorkflowDefinitionInput {
        label: Some(request.label.clone()),
        description: request.description.clone(),
        version: Some(request.version),
        param_schema: request.param_schema.clone(),
        out_schema: request.out_schema.clone(),
        definition: Some(definition_json),
        tags: request.tags,
    };

    let workflow =
        WorkflowDefinitionRepository::update(&mut *tx, existing_workflow.id, update_input).await?;

    // Update the companion action record, or create it if it doesn't exist yet
    // (handles workflows that were created before this fix was deployed)
    update_companion_action(
        &mut tx,
        existing_workflow.id,
        CompanionActionUpdate {
            label: Some(&request.label),
            description: request.description.as_deref(),
            param_schema: request.param_schema.as_ref(),
            out_schema: request.out_schema.as_ref(),
            reference_visibility: request.reference_visibility,
            reference_allowed_pack_refs: Some(&request.reference_allowed_pack_refs),
        },
    )
    .await?;
    let action = ActionRepository::find_by_workflow_def(&mut *tx, existing_workflow.id).await?;
    if action.is_none() {
        create_companion_action(
            &mut tx,
            &workflow_ref,
            pack.id,
            &pack.r#ref,
            &request.label,
            request.description.as_deref(),
            &format!("workflows/{}.workflow.yaml", request.name),
            request.param_schema.as_ref(),
            request.out_schema.as_ref(),
            request
                .reference_visibility
                .unwrap_or(ActionReferenceVisibility::Public),
            &request.reference_allowed_pack_refs,
            existing_workflow.id,
        )
        .await?;
    }
    if let Some(action) = &action {
        let metadata_changed = workflow_behavior_definition(
            &existing_workflow.definition,
            existing_workflow.param_schema.as_ref(),
            existing_workflow.out_schema.as_ref(),
        ) != workflow_behavior_definition(
            &workflow.definition,
            workflow.param_schema.as_ref(),
            workflow.out_schema.as_ref(),
        ) || existing_workflow.param_schema != workflow.param_schema
            || existing_workflow.out_schema != workflow.out_schema
            || previous_companion.as_ref().is_none_or(|previous| {
                previous.param_schema != action.param_schema
                    || previous.out_schema != action.out_schema
                    || previous.reference_visibility != action.reference_visibility
                    || previous.reference_allowed_pack_refs != action.reference_allowed_pack_refs
            });
        if metadata_changed {
            attune_common::repositories::managed_component_readmission::ManagedComponentReadmission::workflow(&mut tx, action.id, workflow_editor(&user)?).await?;
        }
    }
    tx.commit().await?;
    // Managed graph edits live in immutable metadata, not mutable code bytes.
    if action.as_ref().is_none_or(|action| action.is_adhoc) {
        write_workflow_yaml(&packs_base_dir, &file_request.pack_ref, &file_request)
            .await
            .map_err(|_| {
                ApiError::InternalServerError(
                    "Workflow metadata committed; file publication failed".into(),
                )
            })?;
    }

    let response = ApiResponse::with_message(
        WorkflowResponse::from(workflow),
        "Workflow file updated and synced successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Write a workflow definition to disk as YAML
async fn write_workflow_yaml(
    packs_base_dir: &std::path::Path,
    pack_ref: &str,
    request: &SaveWorkflowFileRequest,
) -> Result<(), ApiError> {
    let pack_dir = packs_base_dir.join(pack_ref);
    let actions_dir = pack_dir.join("actions");
    let workflows_dir = actions_dir.join("workflows");

    // Ensure both directories exist
    tokio::fs::create_dir_all(&workflows_dir)
        .await
        .map_err(|e| {
            ApiError::InternalServerError(format!(
                "Failed to create workflows directory '{}': {}",
                workflows_dir.display(),
                e
            ))
        })?;

    // ── 1. Write the workflow file (graph-only: version, vars, tasks, output_map) ──
    let workflow_filename = format!("{}.workflow.yaml", request.name);
    let workflow_filepath = workflows_dir.join(&workflow_filename);

    // Strip action-level fields from the definition — the workflow file should
    // contain only the execution graph. The action YAML is authoritative for
    // ref, label, description, parameters, output, and tags.
    let graph_only = strip_action_level_fields(&request.definition);

    let workflow_yaml = serde_yaml_ng::to_string(&graph_only).map_err(|e| {
        ApiError::BadRequest(format!("Failed to serialize workflow to YAML: {}", e))
    })?;

    let workflow_yaml_with_header = format!(
        "# Workflow execution graph for {}.{}\n\
         # Action-level metadata (ref, label, parameters, output, tags) is defined\n\
         # in the companion action YAML: actions/{}.yaml\n\n{}",
        pack_ref, request.name, request.name, workflow_yaml
    );

    tokio::fs::write(&workflow_filepath, &workflow_yaml_with_header)
        .await
        .map_err(|e| {
            ApiError::InternalServerError(format!(
                "Failed to write workflow file '{}': {}",
                workflow_filepath.display(),
                e
            ))
        })?;

    tracing::info!(
        "Wrote workflow file: {} ({} bytes)",
        workflow_filepath.display(),
        workflow_yaml_with_header.len()
    );

    // ── 2. Write the companion action YAML ──
    let action_filename = format!("{}.yaml", request.name);
    let action_filepath = actions_dir.join(&action_filename);

    let action_yaml = build_action_yaml(pack_ref, request);

    tokio::fs::write(&action_filepath, &action_yaml)
        .await
        .map_err(|e| {
            ApiError::InternalServerError(format!(
                "Failed to write action YAML '{}': {}",
                action_filepath.display(),
                e
            ))
        })?;

    tracing::info!(
        "Wrote action YAML: {} ({} bytes)",
        action_filepath.display(),
        action_yaml.len()
    );

    Ok(())
}

/// Strip action-level fields from a workflow definition JSON, keeping only
/// the execution graph: `version`, `vars`, `tasks`, `output_map`.
///
/// Fields removed: `ref`, `label`, `description`, `parameters`, `output`, `tags`.
fn strip_action_level_fields(definition: &serde_json::Value) -> serde_json::Value {
    if let Some(obj) = definition.as_object() {
        let mut graph = serde_json::Map::new();
        // Keep only graph-level fields
        for key in &["version", "vars", "tasks", "output_map"] {
            if let Some(val) = obj.get(*key) {
                graph.insert((*key).to_string(), val.clone());
            }
        }
        serde_json::Value::Object(graph)
    } else {
        // Shouldn't happen, but pass through if not an object
        definition.clone()
    }
}

/// Build the companion action YAML content for a workflow action.
///
/// This file defines the action-level metadata (ref, label, parameters, etc.)
/// and references the workflow file via `workflow_file`.
fn build_action_yaml(pack_ref: &str, request: &SaveWorkflowFileRequest) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "# Action definition for workflow {}.{}",
        pack_ref, request.name
    ));
    lines.push("# The workflow graph (tasks, transitions, variables) is in:".to_string());
    lines.push(format!(
        "#   actions/workflows/{}.workflow.yaml",
        request.name
    ));
    lines.push(String::new());

    lines.push(format!("ref: {}.{}", pack_ref, request.name));
    lines.push(format!("label: \"{}\"", request.label.replace('"', "\\\"")));
    lines.push(format!("enabled: {}", request.enabled.unwrap_or(true)));
    if let Some(ref desc) = request.description {
        if !desc.is_empty() {
            lines.push(format!("description: \"{}\"", desc.replace('"', "\\\"")));
        }
    }
    lines.push(format!(
        "workflow_file: workflows/{}.workflow.yaml",
        request.name
    ));
    let reference_visibility = request
        .reference_visibility
        .unwrap_or(ActionReferenceVisibility::Public);
    if reference_visibility != ActionReferenceVisibility::Public {
        lines.push(format!(
            "reference_visibility: {}",
            action_reference_visibility_value(reference_visibility)
        ));
    }
    if reference_visibility == ActionReferenceVisibility::Restricted
        && !request.reference_allowed_pack_refs.is_empty()
    {
        lines.push("reference_allowed_pack_refs:".to_string());
        for pack_ref in &request.reference_allowed_pack_refs {
            lines.push(format!("  - {}", pack_ref));
        }
    }

    // Parameters
    if let Some(ref params) = request.param_schema {
        if let Some(obj) = params.as_object() {
            if !obj.is_empty() {
                lines.push(String::new());
                let params_yaml = serde_yaml_ng::to_string(params).unwrap_or_default();
                lines.push("parameters:".to_string());
                // Indent the YAML output under `parameters:`
                for line in params_yaml.lines() {
                    lines.push(format!("  {}", line));
                }
            }
        }
    }

    // Output schema
    if let Some(ref output) = request.out_schema {
        if let Some(obj) = output.as_object() {
            if !obj.is_empty() {
                lines.push(String::new());
                let output_yaml = serde_yaml_ng::to_string(output).unwrap_or_default();
                lines.push("output:".to_string());
                for line in output_yaml.lines() {
                    lines.push(format!("  {}", line));
                }
            }
        }
    }

    // Tags
    if let Some(ref tags) = request.tags {
        if !tags.is_empty() {
            lines.push(String::new());
            lines.push("tags:".to_string());
            for tag in tags {
                lines.push(format!("  - {}", tag));
            }
        }
    }

    lines.push(String::new()); // trailing newline
    lines.join("\n")
}

fn action_reference_visibility_value(visibility: ActionReferenceVisibility) -> &'static str {
    match visibility {
        ActionReferenceVisibility::Public => "public",
        ActionReferenceVisibility::Private => "private",
        ActionReferenceVisibility::Restricted => "restricted",
    }
}

fn validate_reference_visibility_request(
    request: &SaveWorkflowFileRequest,
) -> Result<(), ApiError> {
    let visibility = request
        .reference_visibility
        .unwrap_or(ActionReferenceVisibility::Public);
    if visibility != ActionReferenceVisibility::Restricted
        && !request.reference_allowed_pack_refs.is_empty()
    {
        return Err(ApiError::BadRequest(
            "reference_allowed_pack_refs may only be set when reference_visibility is restricted"
                .to_string(),
        ));
    }
    for pack_ref in &request.reference_allowed_pack_refs {
        RefValidator::validate_pack_ref(pack_ref)
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    }
    Ok(())
}

/// Create a companion action record for a workflow definition.
///
/// This ensures the workflow appears in action lists and the action palette in the
/// workflow builder. The action is linked to the workflow definition via the
/// `workflow_def` FK.
#[allow(clippy::too_many_arguments)]
async fn create_companion_action(
    db: &mut sqlx::PgConnection,
    workflow_ref: &str,
    pack_id: i64,
    pack_ref: &str,
    label: &str,
    description: Option<&str>,
    entrypoint: &str,
    param_schema: Option<&serde_json::Value>,
    out_schema: Option<&serde_json::Value>,
    reference_visibility: ActionReferenceVisibility,
    reference_allowed_pack_refs: &[String],
    workflow_def_id: i64,
) -> Result<(), ApiError> {
    let action_input = CreateActionInput {
        r#ref: workflow_ref.to_string(),
        pack: pack_id,
        pack_ref: pack_ref.to_string(),
        label: label.to_string(),
        description: description.map(|s| s.to_string()),
        entrypoint: entrypoint.to_string(),
        runtime: None,
        enabled: true,
        runtime_version_constraint: None,
        required_worker_runtimes: serde_json::json!({}),
        worker_selector: serde_json::json!({}),
        worker_tolerations: serde_json::json!([]),
        worker_affinity: serde_json::json!({}),
        param_schema: param_schema.cloned(),
        out_schema: out_schema.cloned(),
        is_adhoc: true, // API-authored workflows are not pack release projections.
        accesses_mcp: false,
        default_execution_permission_set_refs: Vec::new(),
        reference_visibility,
        reference_allowed_pack_refs: reference_allowed_pack_refs.to_vec(),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
        timeout_seconds: None,
    };

    let action = ActionRepository::create(&mut *db, action_input)
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to create companion action for workflow '{}': {}",
                workflow_ref,
                e
            );
            ApiError::InternalServerError(format!(
                "Failed to create companion action for workflow: {}",
                e
            ))
        })?;

    // Link the action to the workflow definition (sets workflow_def FK)
    ActionRepository::link_workflow_def(db, action.id, workflow_def_id)
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to link action to workflow definition '{}': {}",
                workflow_ref,
                e
            );
            ApiError::InternalServerError(format!(
                "Failed to link action to workflow definition: {}",
                e
            ))
        })?;

    tracing::info!(
        "Created companion action '{}' (ID: {}) for workflow definition (ID: {})",
        workflow_ref,
        action.id,
        workflow_def_id
    );

    Ok(())
}

async fn validate_workflow_action_references(
    state: &Arc<AppState>,
    pack_ref: &str,
    workflow_ref: &str,
    definition: &serde_json::Value,
) -> Result<(), ApiError> {
    attune_common::pack_registry::runtime_policy::CandidateRuntimePolicyPlan::validate_workflow_targets(definition, pack_ref, workflow_ref, "api/workflows")?;
    let errors =
        attune_common::execution_env::component_environment_errors("workflows", definition);
    if !errors.is_empty() {
        return Err(ApiError::BadRequest(errors.join("; ")));
    }
    let workflow: ParsedWorkflowDefinition =
        serde_json::from_value(definition.clone()).map_err(|e| {
            ApiError::BadRequest(format!(
                "Invalid workflow definition '{}': {}",
                workflow_ref, e
            ))
        })?;

    for action_ref in collect_workflow_action_refs(&workflow) {
        let action = ActionRepository::find_by_ref(&state.db, &action_ref)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "Workflow '{}' references unknown action '{}'",
                    workflow_ref, action_ref
                ))
            })?;
        ensure_action_reference_allowed(&action, Some(pack_ref), "workflow", workflow_ref)?;
    }

    Ok(())
}

/// Update the companion action record for a workflow definition.
///
/// Finds the action linked to the workflow definition and updates its metadata
/// to stay in sync with the workflow definition.
async fn update_companion_action(
    db: &mut sqlx::PgConnection,
    workflow_def_id: i64,
    update: CompanionActionUpdate<'_>,
) -> Result<(), ApiError> {
    let existing_action = ActionRepository::find_by_workflow_def(&mut *db, workflow_def_id)
        .await
        .map_err(|e| {
            tracing::warn!(
                "Failed to look up companion action for workflow_def {}: {}",
                workflow_def_id,
                e
            );
            ApiError::InternalServerError(format!("Failed to look up companion action: {}", e))
        })?;

    if let Some(action) = existing_action {
        let update_input = UpdateActionInput {
            label: update.label.map(|s| s.to_string()),
            description: update.description.map(|s| Patch::Set(s.to_string())),
            entrypoint: None,
            runtime: None,
            enabled: None,
            runtime_version_constraint: None,
            required_worker_runtimes: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            param_schema: update.param_schema.cloned(),
            out_schema: update.out_schema.cloned(),
            parameter_delivery: None,
            parameter_format: None,
            output_format: None,
            accesses_mcp: None,
            default_execution_permission_set_refs: None,
            reference_visibility: update.reference_visibility,
            reference_allowed_pack_refs: update
                .reference_allowed_pack_refs
                .map(|refs| refs.to_vec()),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        };

        ActionRepository::update(db, action.id, update_input)
            .await
            .map_err(|e| {
                tracing::warn!(
                    "Failed to update companion action (ID: {}) for workflow_def {}: {}",
                    action.id,
                    workflow_def_id,
                    e
                );
                ApiError::InternalServerError(format!("Failed to update companion action: {}", e))
            })?;

        tracing::debug!(
            "Updated companion action '{}' (ID: {}) for workflow definition (ID: {})",
            action.r#ref,
            action.id,
            workflow_def_id
        );
    } else {
        tracing::debug!(
            "No companion action found for workflow_def {}; skipping update",
            workflow_def_id
        );
    }

    Ok(())
}

/// Create workflow routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workflows", get(list_workflows).post(create_workflow))
        .route(
            "/workflows/{ref}",
            get(get_workflow)
                .put(update_workflow)
                .delete(delete_workflow),
        )
        .route("/workflows/{ref}/file", put(update_workflow_file))
        .route("/packs/{pack_ref}/workflows", get(list_workflows_by_pack))
        .route("/packs/{pack_ref}/workflow-files", post(save_workflow_file))
}

#[cfg(test)]
mod metadata_comparison_tests {
    use super::workflow_behavior_definition;
    use serde_json::json;

    #[test]
    fn actual_loader_parser_round_trip_matches_label_layout_only_file_edits(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use attune_common::workflow::parser::parse_workflow_yaml;
        let authored = json!({"version":"1.0","vars":{"__chart_meta__":"authored variable"},"tasks":[{
            "name":"run","action":"example.one","input":{"label":"authored input","type":"action","when":null},
            "retry":{"count":2,"delay":0}
        }]});
        // This is the loader's actual parse/serialize path, not a hand-built
        // approximation of the fields emitted by its typed representation.
        let loaded =
            serde_json::to_value(parse_workflow_yaml(&serde_yaml_ng::to_string(&authored)?)?)?;
        assert_ne!(loaded, authored);
        assert_eq!(loaded["tasks"][0]["type"], "action");
        assert_eq!(loaded["tasks"][0]["retry"]["backoff"], "constant");
        let expected = workflow_behavior_definition(&loaded, None, None);
        let mut edited = authored.clone();
        edited["label"] = json!("Presentation only");
        edited["tasks"][0]["__chart_meta__"] = json!({"position":{"x":10,"y":20}});
        assert_eq!(expected, workflow_behavior_definition(&edited, None, None));
        for (field, value) in [
            ("action", json!("example.two")),
            (
                "input",
                json!({"label":"changed","type":"action","when":null}),
            ),
            ("permission_set_refs", json!([])),
            ("worker_selector", json!({})),
            ("worker_tolerations", json!([])),
            ("worker_affinity", json!({})),
            ("when", json!("{{ parameters.enabled }}")),
            ("trace_tag_template", json!("new-trace")),
            (
                "retry",
                json!({"count":2,"delay":0,"backoff":"exponential"}),
            ),
            (
                "retry",
                json!({"count":2,"delay":0,"on_error":"{{ failed() }}"}),
            ),
            ("opaque_extension", json!({"label":"not presentation"})),
        ] {
            let mut changed = authored.clone();
            changed["tasks"][0][field] = value;
            assert_ne!(
                expected,
                workflow_behavior_definition(&changed, None, None),
                "behavioral task field {field} was ignored"
            );
        }
        let mut changed = authored;
        changed["vars"]["__chart_meta__"] = json!("changed variable");
        assert_ne!(expected, workflow_behavior_definition(&changed, None, None));
        Ok(())
    }

    #[test]
    fn file_label_layout_and_default_serialization_do_not_recapture_authority() {
        let schema = json!({"target":{"type":"string"}});
        let old = json!({"tasks":[{"name":"run", "action":"example.one", "allowed_action_refs":["example.one"], "input":{}, "permission_set_refs":null,"with_items":null,"next":[],"retry":{"count":2,"delay":0,"max_delay":null}}],"parameters":null,"vars":{},"output_map":null,"cancellation_policy":"allow_finish"});
        let edited = json!({"ref":"example.flow", "label":"New label", "version":"1.0", "parameters":schema, "tasks":[{"name":"run", "action":"example.one", "allowed_action_refs":["example.one"],"retry":{"count":2,"delay":0},"__chart_meta__":{"position":{"x":10,"y":20}}}]});
        assert_eq!(
            workflow_behavior_definition(&old, Some(&schema), None),
            workflow_behavior_definition(&edited, Some(&schema), None)
        );
    }

    #[test]
    fn comparison_preserves_target_and_permission_ceilings_and_metadata_named_payloads() {
        let original = json!({"tasks":{"__chart_meta__":{"action":"{{ parameters.target }}","allowed_action_refs":["example.one"],"input":{"__chart_meta__":"authored value"}}}});
        let normalized = workflow_behavior_definition(&original, None, None);
        assert!(normalized["tasks"]["__chart_meta__"].is_object());
        for edited in [
            json!({"tasks":{"__chart_meta__":{"action":"{{ parameters.target }}","allowed_action_refs":["example.two"],"input":{"__chart_meta__":"authored value"}}}}),
            json!({"tasks":{"__chart_meta__":{"action":"{{ parameters.target }}","allowed_action_refs":["example.one"],"permission_set_refs":[],"input":{"__chart_meta__":"authored value"}}}}),
            json!({"tasks":{"__chart_meta__":{"action":"{{ parameters.target }}","allowed_action_refs":["example.one"],"input":{"__chart_meta__":"different value"}}}}),
        ] {
            assert_ne!(
                normalized,
                workflow_behavior_definition(&edited, None, None)
            );
        }
    }
}
