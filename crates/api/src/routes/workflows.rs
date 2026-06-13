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

use attune_common::{
    action_visibility::{collect_workflow_action_refs, ensure_action_reference_allowed},
    metadata_cache::repositories::CachedMetadataRepository,
    models::ActionReferenceVisibility,
    repositories::{
        action::{ActionRepository, CreateActionInput, UpdateActionInput},
        pack::PackRepository,
        workflow::{
            CreateWorkflowDefinitionInput, UpdateWorkflowDefinitionInput,
            WorkflowDefinitionRepository, WorkflowSearchFilters,
        },
        Create, Delete, FindByRef, Patch, Update,
    },
    schema::RefValidator,
    workflow::parser::WorkflowDefinition as ParsedWorkflowDefinition,
};

use crate::{
    auth::middleware::RequireAuth,
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
    RequireAuth(_user): RequireAuth,
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

    // All filtering and pagination happen in a single SQL query.
    let filters = WorkflowSearchFilters {
        pack: None,
        pack_ref: search_params.pack_ref.clone(),
        tags,
        search: search_params.search.clone(),
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = WorkflowDefinitionRepository::list_search(&state.db, &filters).await?;

    let paginated_workflows: Vec<WorkflowSummary> =
        result.rows.into_iter().map(WorkflowSummary::from).collect();

    let response = PaginatedResponse::new(paginated_workflows, &pagination, result.total);

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
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Verify pack exists
    let _pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    // All filtering and pagination happen in a single SQL query.
    let filters = WorkflowSearchFilters {
        pack: None,
        pack_ref: Some(pack_ref),
        tags: None,
        search: None,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = WorkflowDefinitionRepository::list_search(&state.db, &filters).await?;

    let paginated_workflows: Vec<WorkflowSummary> =
        result.rows.into_iter().map(WorkflowSummary::from).collect();

    let response = PaginatedResponse::new(paginated_workflows, &pagination, result.total);

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
    RequireAuth(_user): RequireAuth,
    Path(workflow_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let workflow = CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .find_workflow_definition_by_ref(&workflow_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Workflow '{}' not found", workflow_ref)))?;

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
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .put_workflow_definition_best_effort(&workflow)
        .await;

    // Create a companion action record so the workflow appears in action lists
    create_companion_action(
        &state.db,
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
    if let Some(action) = ActionRepository::find_by_ref(&state.db, &workflow.r#ref).await? {
        CachedMetadataRepository::new(&state.db, &state.metadata_cache)
            .put_action_best_effort(&action)
            .await;
    }

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
    RequireAuth(_user): RequireAuth,
    Path(workflow_ref): Path<String>,
    Json(request): Json<UpdateWorkflowRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if workflow exists
    let existing_workflow = WorkflowDefinitionRepository::find_by_ref(&state.db, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Workflow '{}' not found", workflow_ref)))?;
    if let Some(definition) = &request.definition {
        validate_workflow_action_references(
            &state,
            &existing_workflow.pack_ref,
            &existing_workflow.r#ref,
            definition,
        )
        .await?;
    }

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
        WorkflowDefinitionRepository::update(&state.db, existing_workflow.id, update_input).await?;
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .put_workflow_definition_best_effort(&workflow)
        .await;

    // Update the companion action record if it exists
    update_companion_action(
        &state.db,
        existing_workflow.id,
        request.label.as_deref(),
        request.description.as_deref(),
        request.param_schema.as_ref(),
        request.out_schema.as_ref(),
        None,
        None,
    )
    .await?;
    if let Some(action) =
        ActionRepository::find_by_workflow_def(&state.db, existing_workflow.id).await?
    {
        CachedMetadataRepository::new(&state.db, &state.metadata_cache)
            .put_action_best_effort(&action)
            .await;
    }

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
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .evict_workflow_definition_best_effort(&workflow)
        .await;

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
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .put_workflow_definition_best_effort(&workflow)
        .await;

    // Create a companion action record so the workflow appears in action lists and palettes
    let entrypoint = format!("workflows/{}.workflow.yaml", request.name);
    create_companion_action(
        &state.db,
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
    if let Some(action) = ActionRepository::find_by_ref(&state.db, &workflow_ref).await? {
        CachedMetadataRepository::new(&state.db, &state.metadata_cache)
            .put_action_best_effort(&action)
            .await;
    }

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
    RequireAuth(_user): RequireAuth,
    Path(workflow_ref): Path<String>,
    Json(request): Json<SaveWorkflowFileRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    // Check if workflow exists
    let existing_workflow = WorkflowDefinitionRepository::find_by_ref(&state.db, &workflow_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Workflow '{}' not found", workflow_ref)))?;

    // Verify pack exists
    let pack = PackRepository::find_by_ref(&state.db, &request.pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", request.pack_ref)))?;
    validate_reference_visibility_request(&request)?;
    validate_workflow_action_references(&state, &pack.r#ref, &workflow_ref, &request.definition)
        .await?;

    // Write updated YAML file to disk
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);
    write_workflow_yaml(&packs_base_dir, &request.pack_ref, &request).await?;

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
        WorkflowDefinitionRepository::update(&state.db, existing_workflow.id, update_input).await?;
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .put_workflow_definition_best_effort(&workflow)
        .await;

    // Update the companion action record, or create it if it doesn't exist yet
    // (handles workflows that were created before this fix was deployed)
    let entrypoint = format!("workflows/{}.workflow.yaml", request.name);
    ensure_companion_action(
        &state.db,
        existing_workflow.id,
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
    )
    .await?;
    if let Some(action) =
        ActionRepository::find_by_workflow_def(&state.db, existing_workflow.id).await?
    {
        CachedMetadataRepository::new(&state.db, &state.metadata_cache)
            .put_action_best_effort(&action)
            .await;
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
    db: &sqlx::PgPool,
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
        is_adhoc: false,
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

    let action = ActionRepository::create(db, action_input)
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
    db: &sqlx::PgPool,
    workflow_def_id: i64,
    label: Option<&str>,
    description: Option<&str>,
    param_schema: Option<&serde_json::Value>,
    out_schema: Option<&serde_json::Value>,
    reference_visibility: Option<ActionReferenceVisibility>,
    reference_allowed_pack_refs: Option<&[String]>,
) -> Result<(), ApiError> {
    let existing_action = ActionRepository::find_by_workflow_def(db, workflow_def_id)
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
            label: label.map(|s| s.to_string()),
            description: description.map(|s| Patch::Set(s.to_string())),
            entrypoint: None,
            runtime: None,
            enabled: None,
            runtime_version_constraint: None,
            required_worker_runtimes: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            param_schema: param_schema.cloned(),
            out_schema: out_schema.cloned(),
            parameter_delivery: None,
            parameter_format: None,
            output_format: None,
            accesses_mcp: None,
            default_execution_permission_set_refs: None,
            reference_visibility,
            reference_allowed_pack_refs: reference_allowed_pack_refs.map(|refs| refs.to_vec()),
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

/// Ensure a companion action record exists for a workflow definition.
///
/// If the action already exists, update it. If it doesn't exist (e.g., for workflows
/// created before the companion-action fix), create it.
#[allow(clippy::too_many_arguments)]
async fn ensure_companion_action(
    db: &sqlx::PgPool,
    workflow_def_id: i64,
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
) -> Result<(), ApiError> {
    let existing_action = ActionRepository::find_by_workflow_def(db, workflow_def_id)
        .await
        .map_err(|e| {
            ApiError::InternalServerError(format!("Failed to look up companion action: {}", e))
        })?;

    if let Some(action) = existing_action {
        // Update existing companion action
        let update_input = UpdateActionInput {
            label: Some(label.to_string()),
            description: Some(match description {
                Some(description) => Patch::Set(description.to_string()),
                None => Patch::Clear,
            }),
            entrypoint: Some(entrypoint.to_string()),
            runtime: None,
            enabled: None,
            runtime_version_constraint: None,
            required_worker_runtimes: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            param_schema: param_schema.cloned(),
            out_schema: out_schema.cloned(),
            parameter_delivery: None,
            parameter_format: None,
            output_format: None,
            accesses_mcp: None,
            default_execution_permission_set_refs: None,
            reference_visibility: Some(reference_visibility),
            reference_allowed_pack_refs: Some(reference_allowed_pack_refs.to_vec()),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        };

        ActionRepository::update(db, action.id, update_input)
            .await
            .map_err(|e| {
                ApiError::InternalServerError(format!("Failed to update companion action: {}", e))
            })?;

        tracing::debug!(
            "Updated companion action '{}' (ID: {}) for workflow definition (ID: {})",
            action.r#ref,
            action.id,
            workflow_def_id
        );
    } else {
        // Create new companion action (backfill for pre-fix workflows)
        create_companion_action(
            db,
            workflow_ref,
            pack_id,
            pack_ref,
            label,
            description,
            entrypoint,
            param_schema,
            out_schema,
            reference_visibility,
            reference_allowed_pack_refs,
            workflow_def_id,
        )
        .await?;
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
