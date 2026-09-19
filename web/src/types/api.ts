// ============================================================================
// GENERATED API TYPES
// ============================================================================
// This file re-exports types from the auto-generated OpenAPI client.
// Use these instead of defining manual types to ensure schema consistency.
//
// To regenerate the API client: npm run generate:api
// ============================================================================

// Re-export generated types from OpenAPI client
export type {
  // Authentication
  LoginRequest,
  RegisterRequest,
  RefreshTokenRequest,
  ChangePasswordRequest,
  UserInfo,
  CurrentUserResponse,

  // Packs
  PackResponse,
  PackSummary,
  CreatePackRequest,
  UpdatePackRequest,

  // Actions
  ActionResponse,
  ActionSummary,
  CreateActionRequest,
  UpdateActionRequest,

  // Rules
  RuleResponse,
  RuleSummary,
  CreateRuleRequest,
  UpdateRuleRequest,

  // Triggers
  TriggerResponse,
  TriggerSummary,
  CreateTriggerRequest,
  UpdateTriggerRequest,

  // Sensors
  SensorResponse,
  SensorSummary,
  CreateSensorRequest,
  UpdateSensorRequest,

  // Executions
  ExecutionResponse,
  ExecutionSummary,

  // Events
  EventResponse,
  EventSummary,

  // Workflows
  WorkflowResponse,
  WorkflowSummary,
  CreateWorkflowRequest,
  UpdateWorkflowRequest,

  // Inquiries
  InquiryResponse,
  InquirySummary,
  CreateInquiryRequest,
  InquiryRespondRequest,

  // Secrets/Keys
  KeyResponse,
  KeySummary,
  CreateKeyRequest,
  UpdateKeyRequest,

  // Enforcements
  EnforcementResponse,
  EnforcementSummary,

  // Paginated Responses
  PaginatedResponse_PackSummary,
  PaginatedResponse_ActionSummary,
  PaginatedResponse_RuleSummary,
  PaginatedResponse_TriggerSummary,
  PaginatedResponse_SensorSummary,
  PaginatedResponse_ExecutionSummary,
  PaginatedResponse_EventSummary,
  PaginatedResponse_WorkflowSummary,
  PaginatedResponse_InquirySummary,
  PaginatedResponse_KeySummary,
  PaginatedResponse_EnforcementSummary,

  // API Response Wrappers
  ApiResponse_PackResponse,
  ApiResponse_ActionResponse,
  ApiResponse_RuleResponse,
  ApiResponse_TriggerResponse,
  ApiResponse_SensorResponse,
  ApiResponse_ExecutionResponse,
  ApiResponse_EventResponse,
  ApiResponse_WorkflowResponse,
  ApiResponse_InquiryResponse,
  ApiResponse_KeyResponse,
  ApiResponse_EnforcementResponse,
  ApiResponse_CurrentUserResponse,
  ApiResponse_TokenResponse,
  ApiResponse_QueueStatsResponse,

  // Enums
  ExecutionStatus,
  EnforcementStatus,
  InquiryStatus,
  OwnerType,
  EnforcementCondition,

  // Other
  HealthResponse,
  SuccessResponse,
  QueueStatsResponse,
  PaginationMeta,
  PackWorkflowSyncResponse,
  PackWorkflowValidationResponse,
  WorkflowSyncResult,
} from "@/api";

// Re-export services for convenience
export {
  AuthService,
  PacksService,
  ActionsService,
  RulesService,
  TriggersService,
  SensorsService,
  ExecutionsService,
  EventsService,
  WorkflowsService,
  InquiriesService,
  SecretsService,
  EnforcementsService,
  HealthService,
  ApiError,
} from "@/api";

// ============================================================================
// TYPE ALIASES FOR BACKWARD COMPATIBILITY
// ============================================================================
// These provide backward compatibility with existing code.
// Prefer using the generated types directly.
// ============================================================================

import type { CurrentUserResponse } from "@/api";

/**
 * @deprecated Use CurrentUserResponse from '@/api' instead
 */
export type User = CurrentUserResponse;

/**
 * Common API Response wrapper (generic)
 * Note: Generated types include specific ApiResponse_* types
 */
export interface ApiResponse<T> {
  data: T;
  message?: string;
}

/**
 * Generic Paginated Response
 * Note: Generated types include specific PaginatedResponse_* types
 */
export interface PaginatedResponse<T> {
  items: T[];
  total: number;
  page: number;
  page_size: number;
}

/**
 * Error response structure
 */
export interface ErrorResponse {
  error: string;
  message: string;
  details?: Record<string, unknown>;
}

// ============================================================================
// DEPRECATED MANUAL TYPES - USE GENERATED TYPES INSTEAD
// ============================================================================
// These types are kept for backward compatibility only.
// Migrate to using generated types from '@/api'
// ============================================================================

import type {
  PackResponse,
  ActionResponse,
  RuleResponse,
  TriggerResponse,
  SensorResponse,
  ExecutionResponse,
  EventResponse,
  EnforcementResponse,
  InquiryResponse,
  WorkflowResponse,
} from "@/api";

/**
 * @deprecated Use PackResponse from '@/api' instead
 */
export type Pack = PackResponse;

/**
 * @deprecated Use ActionResponse from '@/api' instead
 */
export type Action = ActionResponse;

/**
 * @deprecated Use RuleResponse from '@/api' instead
 */
export type Rule = RuleResponse;

/**
 * @deprecated Use TriggerResponse from '@/api' instead
 */
export type Trigger = TriggerResponse;

/**
 * @deprecated Use SensorResponse from '@/api' instead
 */
export type Sensor = SensorResponse;

/**
 * @deprecated Use ExecutionResponse from '@/api' instead
 */
export type Execution = ExecutionResponse;

/**
 * @deprecated Use EventResponse from '@/api' instead
 */
export type Event = EventResponse;

/**
 * @deprecated Use EnforcementResponse from '@/api' instead
 */
export type Enforcement = EnforcementResponse;

/**
 * @deprecated Use InquiryResponse from '@/api' instead
 */
export type Inquiry = InquiryResponse;

/**
 * @deprecated Use WorkflowResponse from '@/api' instead
 */
export type Workflow = WorkflowResponse;

// Notification Types
export interface Notification {
  type: "execution_update" | "inquiry_created" | "event_created";
  entity_type: string;
  entity_id: number;
  timestamp: string;
  data: Record<string, unknown>;
}
