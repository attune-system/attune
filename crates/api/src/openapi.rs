//! OpenAPI specification and documentation

use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use crate::dto::{
    action::{
        ActionResponse, ActionSearchHit, ActionSummary, CreateActionRequest, QueueStatsResponse,
        UpdateActionRequest,
    },
    auth::{
        AuthSettingsResponse, ChangePasswordRequest, CurrentUserResponse,
        EffectivePermissionResponse, LoginRequest, ProviderProfileResponse, RefreshTokenRequest,
        RegisterRequest, TokenLoginRequest, TokenResponse, UpdateCurrentUserRequest,
    },
    common::{ApiResponse, PaginatedResponse, PaginationMeta, SuccessResponse},
    dashboard::{
        CloneDashboardRequest, CreateDashboardRequest, DashboardDataRequest, DashboardDataResponse,
        DashboardListItemResponse, DashboardMetadataResponse, DashboardSourceCatalogResponse,
        DashboardSourceContractResponse, DashboardSourceParamSchemaResponse,
        PreviewDashboardRequest, UpdateDashboardRequest,
    },
    event::{EnforcementResponse, EnforcementSummary, EventResponse, EventSummary},
    execution::{ExecutionRescheduleResponse, ExecutionResponse, ExecutionSummary},
    inquiry::{
        CreateInquiryRequest, InquiryRespondRequest, InquiryResponse, InquirySummary,
        UpdateInquiryRequest,
    },
    key::{CreateKeyRequest, KeyResponse, KeySummary, UpdateKeyRequest},
    pack::{
        CreatePackRequest, InstallPackRequest, PackInstallResponse, PackResponse, PackSummary,
        PackWorkflowSyncResponse, PackWorkflowValidationResponse, RegisterPackRequest,
        UpdatePackRequest, WorkflowSyncResult,
    },
    permission::{
        CreateIdentityRequest, CreateIdentityRoleAssignmentRequest, CreateIntegrationTokenRequest,
        CreateIntegrationTokenResponse, CreatePermissionAssignmentRequest,
        CreatePermissionSetRoleAssignmentRequest, IdentityResponse, IdentityRoleAssignmentResponse,
        IdentitySummary, IntegrationTokenResponse, PermissionAssignmentResponse,
        PermissionSetRoleAssignmentResponse, PermissionSetSummary, RevokeIntegrationTokenRequest,
        UpdateIdentityRequest, UpdatePermissionSetRequest,
    },
    policy::{
        ConcurrencyPolicyRequest, ConcurrencyPolicyResponse, CreatePolicyRequest, PolicyResponse,
        PolicyScopeRequest, PolicyScopeResponse, PolicySummary, QuotaPolicyRequest,
        QuotaPolicyResponse, RateLimitPolicyRequest, RateLimitPolicyResponse, UpdatePolicyRequest,
    },
    rule::{CreateRuleRequest, RuleResponse, RuleSummary, UpdateRuleRequest},
    runtime::{CreateRuntimeRequest, RuntimeResponse, RuntimeSummary, UpdateRuntimeRequest},
    trace::{TraceReportResponse, TraceWorkQueueDispatchSummary},
    trigger::{
        CreateSensorRequest, CreateTriggerRequest, SensorResponse, SensorSummary, TriggerResponse,
        TriggerSummary, UpdateSensorRequest, UpdateTriggerRequest,
    },
    webhook::{WebhookReceiverRequest, WebhookReceiverResponse},
    work_queue::{
        ApplyWorkQueueItemsRequest, ApplyWorkQueueItemsResponse, CreateWorkQueueRequest,
        EnqueueWorkQueueItemRequest, PreviewWorkQueueItemsRequest, PreviewWorkQueueItemsResponse,
        UpdateWorkQueueItemRequest, UpdateWorkQueueRequest, WorkQueueItemBulkOperation,
        WorkQueueItemJsonPathSelector, WorkQueueItemResponse, WorkQueueResponse, WorkQueueSummary,
    },
    worker::{
        CordonWorkerRequest, WorkerHealthState, WorkerLoadSnapshot, WorkerRuntimeSupport,
        WorkerSummary,
    },
    workflow::{CreateWorkflowRequest, UpdateWorkflowRequest, WorkflowResponse, WorkflowSummary},
};

use crate::dto::audit::{AuditEventResponse, AuditEventSummary};
use attune_common::audit::{AuditCategory, AuditOutcome};

/// OpenAPI documentation structure
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Attune API",
        version = "0.1.0",
        description = "Event-driven automation and orchestration platform API",
        contact(
            name = "Attune Team",
            url = "https://github.com/yourusername/attune"
        ),
        license(
            name = "MIT",
            url = "https://opensource.org/licenses/MIT"
        )
    ),
    servers(
        (url = "http://localhost:8080", description = "Local development server"),
        (url = "https://api.attune.example.com", description = "Production server")
    ),
    paths(
        // Health check
        crate::routes::health::health,
        crate::routes::health::health_detailed,
        crate::routes::health::readiness,
        crate::routes::health::liveness,

        // Authentication
        crate::routes::auth::auth_settings,
        crate::routes::auth::login,
        crate::routes::auth::token_login,
        crate::routes::auth::ldap_login,
        crate::routes::auth::register,
        crate::routes::auth::refresh_token,
        crate::routes::auth::get_current_user,
        crate::routes::auth::update_current_user,
        crate::routes::auth::change_password,

        // Packs
        crate::routes::packs::list_packs,
        crate::routes::packs::get_pack,
        crate::routes::packs::create_pack,
        crate::routes::packs::update_pack,
        crate::routes::packs::delete_pack,
        crate::routes::packs::register_pack,
        crate::routes::packs::install_pack,
        crate::routes::packs::sync_pack_workflows,
        crate::routes::packs::validate_pack_workflows,
        crate::routes::packs::test_pack,
        crate::routes::packs::get_pack_test_history,
        crate::routes::packs::get_pack_latest_test,

        // Actions
        crate::routes::actions::list_actions,
        crate::routes::actions::list_actions_by_pack,
        crate::routes::actions::search_actions,
        crate::routes::actions::get_action,
        crate::routes::actions::create_action,
        crate::routes::actions::update_action,
        crate::routes::actions::delete_action,
        crate::routes::actions::get_queue_stats,

        // Policies
        crate::routes::policies::list_policies,
        crate::routes::policies::list_policies_by_pack,
        crate::routes::policies::list_policies_by_action,
        crate::routes::policies::get_policy,
        crate::routes::policies::create_policy,
        crate::routes::policies::update_policy,
        crate::routes::policies::delete_policy,

        // Runtimes
        crate::routes::runtimes::list_runtimes,
        crate::routes::runtimes::list_runtimes_by_pack,
        crate::routes::runtimes::get_runtime,
        crate::routes::runtimes::create_runtime,
        crate::routes::runtimes::update_runtime,
        crate::routes::runtimes::delete_runtime,
        crate::routes::workers::list_workers,
        crate::routes::workers::get_worker,
        crate::routes::workers::cordon_worker,
        crate::routes::workers::uncordon_worker,
        crate::routes::retention::get_retention_config,
        crate::routes::retention::update_retention_config,

        // Work queues
        crate::routes::work_queues::list_queues,
        crate::routes::work_queues::list_queues_by_pack,
        crate::routes::work_queues::get_queue,
        crate::routes::work_queues::create_queue,
        crate::routes::work_queues::update_queue,
        crate::routes::work_queues::delete_queue,
        crate::routes::work_queues::list_queue_items,
        crate::routes::work_queues::preview_queue_items_by_selector,
        crate::routes::work_queues::apply_queue_items_by_selector,
        crate::routes::work_queues::enqueue_queue_item,
        crate::routes::work_queues::update_queue_item,
        crate::routes::work_queues::delete_queue_item,

        // Triggers
        crate::routes::triggers::list_triggers,
        crate::routes::triggers::list_enabled_triggers,
        crate::routes::triggers::list_triggers_by_pack,
        crate::routes::triggers::get_trigger,
        crate::routes::triggers::create_trigger,
        crate::routes::triggers::update_trigger,
        crate::routes::triggers::delete_trigger,
        crate::routes::triggers::enable_trigger,
        crate::routes::triggers::disable_trigger,

        // Sensors
        crate::routes::triggers::list_sensors,
        crate::routes::triggers::list_enabled_sensors,
        crate::routes::triggers::list_sensors_by_pack,
        crate::routes::triggers::list_sensors_by_trigger,
        crate::routes::triggers::get_sensor,
        crate::routes::triggers::create_sensor,
        crate::routes::triggers::update_sensor,
        crate::routes::triggers::delete_sensor,
        crate::routes::triggers::enable_sensor,
        crate::routes::triggers::disable_sensor,

        // Rules
        crate::routes::rules::list_rules,
        crate::routes::rules::list_enabled_rules,
        crate::routes::rules::list_rules_by_pack,
        crate::routes::rules::list_rules_by_action,
        crate::routes::rules::list_rules_by_trigger,
        crate::routes::rules::get_rule,
        crate::routes::rules::create_rule,
        crate::routes::rules::update_rule,
        crate::routes::rules::delete_rule,
        crate::routes::rules::enable_rule,
        crate::routes::rules::disable_rule,

        // Executions
        crate::routes::executions::create_execution,
        crate::routes::executions::list_executions,
        crate::routes::executions::get_execution,
        crate::routes::executions::list_executions_by_status,
        crate::routes::executions::list_executions_by_enforcement,
        crate::routes::executions::get_execution_stats,
        crate::routes::executions::cancel_execution,
        crate::routes::executions::reschedule_execution,

        // Events
        crate::routes::events::list_events,
        crate::routes::events::get_event,

        // Enforcements
        crate::routes::events::list_enforcements,
        crate::routes::events::get_enforcement,

        // Traces
        crate::routes::traces::get_trace_report,

        // Inquiries
        crate::routes::inquiries::list_inquiries,
        crate::routes::inquiries::get_inquiry,
        crate::routes::inquiries::list_inquiries_by_status,
        crate::routes::inquiries::list_inquiries_by_execution,
        crate::routes::inquiries::create_inquiry,
        crate::routes::inquiries::update_inquiry,
        crate::routes::inquiries::respond_to_inquiry,
        crate::routes::inquiries::delete_inquiry,

        // Keys/Secrets
        crate::routes::keys::list_keys,
        crate::routes::keys::get_key,
        crate::routes::keys::create_key,
        crate::routes::keys::update_key,
        crate::routes::keys::delete_key,

        // Permissions
        crate::routes::permissions::list_identities,
        crate::routes::permissions::get_identity,
        crate::routes::permissions::create_identity,
        crate::routes::permissions::update_identity,
        crate::routes::permissions::delete_identity,
        crate::routes::permissions::list_permission_sets,
        crate::routes::permissions::update_permission_set,
        crate::routes::permissions::list_identity_permissions,
        crate::routes::permissions::create_permission_assignment,
        crate::routes::permissions::delete_permission_assignment,
        crate::routes::permissions::create_identity_role_assignment,
        crate::routes::permissions::delete_identity_role_assignment,
        crate::routes::permissions::create_permission_set_role_assignment,
        crate::routes::permissions::delete_permission_set_role_assignment,
        crate::routes::permissions::freeze_identity,
        crate::routes::permissions::unfreeze_identity,
        crate::routes::permissions::list_integration_tokens,
        crate::routes::permissions::create_integration_token,
        crate::routes::permissions::revoke_integration_token,
        crate::routes::permissions::delete_integration_token,

        // Workflows
        crate::routes::workflows::list_workflows,
        crate::routes::workflows::list_workflows_by_pack,
        crate::routes::workflows::get_workflow,
        crate::routes::workflows::create_workflow,
        crate::routes::workflows::update_workflow,
        crate::routes::workflows::delete_workflow,

        // Dashboards
        crate::routes::dashboards::list_dashboards,
        crate::routes::dashboards::get_dashboard_source_catalog,
        crate::routes::dashboards::create_dashboard,
        crate::routes::dashboards::get_dashboard,
        crate::routes::dashboards::update_dashboard,
        crate::routes::dashboards::delete_dashboard,
        crate::routes::dashboards::clone_dashboard,
        crate::routes::dashboards::preview_dashboard,
        crate::routes::dashboards::get_dashboard_data,

        // Webhooks
        crate::routes::webhooks::enable_webhook,
        crate::routes::webhooks::disable_webhook,
        crate::routes::webhooks::regenerate_webhook_key,
        crate::routes::webhooks::receive_webhook,

        // Agent
        crate::routes::agent::download_agent_binary,
        crate::routes::agent::agent_info,

        // Audit log
        crate::routes::audit::list_audit_events,
        crate::routes::audit::get_audit_event,
        crate::routes::audit::get_audit_events_by_request,
    ),
    components(
        schemas(
            // Common types
            ApiResponse<TokenResponse>,
            ApiResponse<AuthSettingsResponse>,
            ApiResponse<CurrentUserResponse>,
            ApiResponse<PackResponse>,
            ApiResponse<PackInstallResponse>,
            ApiResponse<ActionResponse>,
            ApiResponse<PolicyResponse>,
            ApiResponse<RuntimeResponse>,
            ApiResponse<TriggerResponse>,
            ApiResponse<SensorResponse>,
            ApiResponse<RuleResponse>,
            ApiResponse<ExecutionResponse>,
            ApiResponse<EventResponse>,
            ApiResponse<EnforcementResponse>,
            ApiResponse<InquiryResponse>,
            ApiResponse<KeyResponse>,
            ApiResponse<IdentityResponse>,
            ApiResponse<PermissionAssignmentResponse>,
            ApiResponse<WorkflowResponse>,
            ApiResponse<DashboardMetadataResponse>,
            ApiResponse<Vec<DashboardListItemResponse>>,
            ApiResponse<DashboardSourceCatalogResponse>,
            CreateDashboardRequest,
            UpdateDashboardRequest,
            CloneDashboardRequest,
            PreviewDashboardRequest,
            DashboardSourceContractResponse,
            DashboardSourceParamSchemaResponse,
            DashboardDataResponse,
            ApiResponse<QueueStatsResponse>,
            ApiResponse<WorkQueueResponse>,
            ApiResponse<WorkQueueItemResponse>,
            ApiResponse<PreviewWorkQueueItemsResponse>,
            ApiResponse<ApplyWorkQueueItemsResponse>,
            PaginatedResponse<PackSummary>,
            PaginatedResponse<ActionSummary>,
            PaginatedResponse<PolicySummary>,
            PaginatedResponse<RuntimeSummary>,
            PaginatedResponse<WorkerSummary>,
            PaginatedResponse<WorkQueueSummary>,
            PaginatedResponse<WorkQueueItemResponse>,
            PaginatedResponse<TriggerSummary>,
            PaginatedResponse<SensorSummary>,
            PaginatedResponse<RuleSummary>,
            PaginatedResponse<ExecutionSummary>,
            PaginatedResponse<EventSummary>,
            PaginatedResponse<EnforcementSummary>,
            PaginatedResponse<InquirySummary>,
            PaginatedResponse<KeySummary>,
            PaginatedResponse<IdentitySummary>,
            PaginatedResponse<WorkflowSummary>,
            PaginationMeta,
            SuccessResponse,

            // Auth DTOs
            LoginRequest,
            TokenLoginRequest,
            crate::routes::auth::LdapLoginRequest,
            RegisterRequest,
            RefreshTokenRequest,
            ChangePasswordRequest,
            UpdateCurrentUserRequest,
            TokenResponse,
            CurrentUserResponse,
            ProviderProfileResponse,
            EffectivePermissionResponse,

            // Pack DTOs
            CreatePackRequest,
            UpdatePackRequest,
            RegisterPackRequest,
            InstallPackRequest,
            PackResponse,
            PackSummary,
            PackInstallResponse,
            PackWorkflowSyncResponse,
            PackWorkflowValidationResponse,
            WorkflowSyncResult,
            attune_common::models::pack_test::PackTestResult,
            attune_common::models::pack_test::PackTestExecution,
            attune_common::models::pack_test::TestSuiteResult,
            attune_common::models::pack_test::TestCaseResult,
            attune_common::models::pack_test::TestStatus,
            attune_common::models::pack_test::PackTestSummary,
            PaginatedResponse<attune_common::models::pack_test::PackTestSummary>,

            // Permission DTOs
            CreateIdentityRequest,
            UpdateIdentityRequest,
            IdentityResponse,
            IntegrationTokenResponse,
            CreateIntegrationTokenRequest,
            CreateIntegrationTokenResponse,
            RevokeIntegrationTokenRequest,
            PermissionSetSummary,
            UpdatePermissionSetRequest,
            PermissionAssignmentResponse,
            CreatePermissionAssignmentRequest,
            CreateIdentityRoleAssignmentRequest,
            IdentityRoleAssignmentResponse,
            CreatePermissionSetRoleAssignmentRequest,
            PermissionSetRoleAssignmentResponse,

            // Runtime DTOs
            CreateRuntimeRequest,
            UpdateRuntimeRequest,
            RuntimeResponse,
            RuntimeSummary,
            TraceReportResponse,
            TraceWorkQueueDispatchSummary,
            WorkerLoadSnapshot,
            WorkerRuntimeSupport,
            CordonWorkerRequest, WorkerHealthState, WorkerSummary,
            attune_common::config::RetentionConfig,
            attune_common::config::RetentionTargetsConfig,
            attune_common::config::RetentionTargetConfig,
            IdentitySummary,
            CreateWorkQueueRequest,
            EnqueueWorkQueueItemRequest,
            PreviewWorkQueueItemsRequest,
            PreviewWorkQueueItemsResponse,
            ApplyWorkQueueItemsRequest,
            ApplyWorkQueueItemsResponse,
            UpdateWorkQueueRequest,
            UpdateWorkQueueItemRequest,
            WorkQueueItemBulkOperation,
            WorkQueueItemJsonPathSelector,
            WorkQueueItemResponse,
            WorkQueueResponse,
            WorkQueueSummary,

            // Action DTOs
            CreateActionRequest,
            UpdateActionRequest,
            ActionResponse,
            ActionSummary,
            ActionSearchHit,
            PaginatedResponse<ActionSearchHit>,
            QueueStatsResponse,

            // Policy DTOs
            CreatePolicyRequest,
            UpdatePolicyRequest,
            PolicyResponse,
            PolicySummary,
            PolicyScopeRequest,
            PolicyScopeResponse,
            ConcurrencyPolicyRequest,
            ConcurrencyPolicyResponse,
            RateLimitPolicyRequest,
            RateLimitPolicyResponse,
            QuotaPolicyRequest,
            QuotaPolicyResponse,

            // Trigger DTOs
            CreateTriggerRequest,
            UpdateTriggerRequest,
            TriggerResponse,
            TriggerSummary,

            // Sensor DTOs
            CreateSensorRequest,
            UpdateSensorRequest,
            SensorResponse,
            SensorSummary,

            // Rule DTOs
            CreateRuleRequest,
            UpdateRuleRequest,
            RuleResponse,
            RuleSummary,

            // Execution DTOs
            ExecutionResponse,
            ExecutionRescheduleResponse,
            ExecutionSummary,

            // Event DTOs
            EventResponse,
            EventSummary,

            // Enforcement DTOs
            EnforcementResponse,
            EnforcementSummary,

            // Inquiry DTOs
            CreateInquiryRequest,
            UpdateInquiryRequest,
            InquiryRespondRequest,
            InquiryResponse,
            InquirySummary,

            // Key/Secret DTOs
            CreateKeyRequest,
            UpdateKeyRequest,
            KeyResponse,
            KeySummary,

            // Workflow DTOs
            CreateWorkflowRequest,
            UpdateWorkflowRequest,
            DashboardDataRequest,
            WorkflowResponse,
            WorkflowSummary,

            // Webhook DTOs
            WebhookReceiverRequest,
            WebhookReceiverResponse,
            ApiResponse<WebhookReceiverResponse>,

            // Agent DTOs
            crate::routes::agent::AgentBinaryInfo,
            crate::routes::agent::AgentArchInfo,

            // Audit DTOs
            AuditCategory,
            AuditOutcome,
            AuditEventResponse,
            AuditEventSummary,
            ApiResponse<AuditEventResponse>,
            ApiResponse<Vec<AuditEventResponse>>,
            PaginatedResponse<AuditEventSummary>,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "health", description = "Health check endpoints"),
        (name = "auth", description = "Authentication and authorization endpoints"),
        (name = "packs", description = "Pack management endpoints"),
        (name = "actions", description = "Action management endpoints"),
        (name = "policies", description = "Execution policy management endpoints"),
        (name = "triggers", description = "Trigger management endpoints"),
        (name = "sensors", description = "Sensor management endpoints"),
        (name = "rules", description = "Rule management endpoints"),
        (name = "executions", description = "Execution query endpoints"),
        (name = "inquiries", description = "Inquiry (human-in-the-loop) endpoints"),
        (name = "events", description = "Event query endpoints"),
        (name = "enforcements", description = "Enforcement query endpoints"),
        (name = "secrets", description = "Secret management endpoints"),
        (name = "workers", description = "Worker inventory and load endpoints"),
        (name = "queues", description = "Work queue definition endpoints"),
        (name = "workflows", description = "Workflow management endpoints"),
        (name = "webhooks", description = "Webhook management and receiver endpoints"),
        (name = "agent", description = "Agent binary distribution endpoints"),
        (name = "audit", description = "Audit log query endpoints"),
    )
)]
pub struct ApiDoc;

/// Security scheme modifier to add JWT Bearer authentication
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .description(Some(
                            "JWT access token obtained from /auth/login or /auth/register",
                        ))
                        .build(),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openapi_spec_generation() {
        let doc = ApiDoc::openapi();

        // Verify basic info
        assert_eq!(doc.info.title, "Attune API");
        assert_eq!(doc.info.version, "0.1.0");

        // Verify we have components
        assert!(doc.components.is_some());

        // Verify we have security schemes
        let components = doc.components.unwrap();
        assert!(components.security_schemes.contains_key("bearer_auth"));
    }

    #[test]
    fn test_openapi_endpoint_count() {
        let doc = ApiDoc::openapi();

        // Count all paths in the OpenAPI spec
        let path_count = doc.paths.paths.len();

        // Count all operations (methods on paths)
        let operation_count: usize = doc
            .paths
            .paths
            .values()
            .map(|path_item| {
                let mut count = 0;
                if path_item.get.is_some() {
                    count += 1;
                }
                if path_item.post.is_some() {
                    count += 1;
                }
                if path_item.put.is_some() {
                    count += 1;
                }
                if path_item.delete.is_some() {
                    count += 1;
                }
                if path_item.patch.is_some() {
                    count += 1;
                }
                count
            })
            .sum();

        // We have 57 unique paths with 81 total operations (HTTP methods)
        // This test ensures we don't accidentally remove endpoints
        assert!(
            path_count >= 59,
            "Expected at least 59 unique API paths, found {}",
            path_count
        );

        assert!(
            operation_count >= 83,
            "Expected at least 83 API operations, found {}",
            operation_count
        );

        println!("Total API paths: {}", path_count);
        println!("Total API operations: {}", operation_count);
    }

    #[test]
    fn test_auth_endpoints_registered() {
        let doc = ApiDoc::openapi();

        let expected_auth_paths = vec![
            "/auth/settings",
            "/auth/login",
            "/auth/ldap/login",
            "/auth/register",
            "/auth/refresh",
            "/auth/me",
            "/auth/change-password",
        ];

        for path in &expected_auth_paths {
            assert!(
                doc.paths.paths.contains_key(*path),
                "Expected auth endpoint {} to be registered in OpenAPI spec, but it was missing. \
                 Registered paths: {:?}",
                path,
                doc.paths.paths.keys().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_ldap_login_request_schema_registered() {
        let doc = ApiDoc::openapi();

        let components = doc.components.as_ref().expect("components should exist");

        assert!(
            components.schemas.contains_key("LdapLoginRequest"),
            "Expected LdapLoginRequest schema to be registered in OpenAPI components. \
             Registered schemas: {:?}",
            components.schemas.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_work_queue_paths_and_schemas_registered() {
        let doc = ApiDoc::openapi();

        for path in [
            "/api/v1/queues",
            "/api/v1/queues/{ref}",
            "/api/v1/queues/{ref}/items",
            "/api/v1/queues/{ref}/items/{item_id}",
            "/api/v1/packs/{pack_ref}/queues",
        ] {
            assert!(
                doc.paths.paths.contains_key(path),
                "expected work queue path '{}' to exist in OpenAPI spec",
                path
            );
        }

        let components = doc.components.as_ref().expect("components should exist");
        for schema in [
            "CreateWorkQueueRequest",
            "UpdateWorkQueueRequest",
            "EnqueueWorkQueueItemRequest",
            "UpdateWorkQueueItemRequest",
            "WorkQueueResponse",
            "WorkQueueSummary",
            "WorkQueueItemResponse",
        ] {
            assert!(
                components.schemas.contains_key(schema),
                "expected work queue schema '{}' to exist in OpenAPI components",
                schema
            );
        }
    }

    #[test]
    fn test_create_action_request_schema_is_client_generator_safe() {
        let doc = ApiDoc::openapi();
        let spec = serde_json::to_value(&doc).expect("OpenAPI spec should serialize");

        assert!(
            spec.pointer("/components/schemas/CreateActionRequest")
                .is_some(),
            "CreateActionRequest schema should be present for POST /api/v1/actions"
        );

        assert!(
            spec.pointer(
                "/components/schemas/CreateActionRequest/properties/worker_affinity/default"
            )
            .is_none(),
            "worker_affinity must not declare a schema default; openapi-python-client rejects defaults on oneOf model properties"
        );
    }

    #[test]
    fn test_queue_item_filters_are_query_parameters() {
        let doc = ApiDoc::openapi();
        let spec = serde_json::to_value(&doc).expect("OpenAPI spec should serialize");

        let params = spec
            .pointer("/paths/~1api~1v1~1queues~1{ref}~1items/get/parameters")
            .and_then(|value| value.as_array())
            .expect("queue item list parameters should be an array");

        let item_key = params
            .iter()
            .find(|param| param.get("name").and_then(|name| name.as_str()) == Some("item_key"))
            .expect("item_key filter parameter should be documented");

        assert_eq!(
            item_key.get("in").and_then(|value| value.as_str()),
            Some("query"),
            "item_key is an optional list filter and must not be emitted as a path parameter"
        );

        let schema_type = item_key
            .pointer("/schema/type")
            .expect("item_key parameter should have a schema type");
        assert!(
            schema_type == "string"
                || schema_type
                    .as_array()
                    .is_some_and(|types| types.iter().any(|value| value == "string")),
            "item_key should remain string-compatible for OpenAPI clients"
        );
    }
}
