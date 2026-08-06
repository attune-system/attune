/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateExecutionRequest } from "../models/CreateExecutionRequest";
import type { ExecutionResponse } from "../models/ExecutionResponse";
import type { ExecutionStatus } from "../models/ExecutionStatus";
import type { PaginatedResponse_ExecutionSummary } from "../models/PaginatedResponse_ExecutionSummary";
import type { RetentionPolicyType } from "../models/RetentionPolicyType";
import type { WorkflowCacheIterationState } from "../models/WorkflowCacheIterationState";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class ExecutionsService {
  /**
   * List executions by enforcement ID
   * @returns PaginatedResponse_ExecutionSummary List of executions for enforcement
   * @throws ApiError
   */
  public static listExecutionsByEnforcement({
    enforcementId,
    page,
    pageSize,
  }: {
    /**
     * Enforcement ID
     */
    enforcementId: number;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_ExecutionSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/enforcements/{enforcement_id}/executions",
      path: {
        enforcement_id: enforcementId,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        500: `Internal server error`,
      },
    });
  }
  /**
   * List all executions with pagination and optional filters
   * @returns PaginatedResponse_ExecutionSummary List of executions
   * @throws ApiError
   */
  public static listExecutions({
    status,
    actionRef,
    packName,
    ruleRef,
    triggerRef,
    traceTag,
    executor,
    resultContains,
    enforcement,
    parent,
    topLevelOnly,
    includeTotal,
    page,
    perPage,
  }: {
    /**
     * Filter by execution status
     */
    status?: null | ExecutionStatus;
    /**
     * Filter by action reference.
     * Supports exact refs and `<pack>.*` wildcards such as `core.*`.
     */
    actionRef?: string | null;
    /**
     * Filter by pack name
     */
    packName?: string | null;
    /**
     * Filter by rule reference.
     * Supports exact refs and `<pack>.*` wildcards such as `core.*`.
     */
    ruleRef?: string | null;
    /**
     * Filter by trigger reference.
     * Supports exact refs and `<pack>.*` wildcards such as `core.*`.
     */
    triggerRef?: string | null;
    /**
     * Filter by exact trace tag.
     */
    traceTag?: string | null;
    /**
     * Filter by executor ID
     */
    executor?: number | null;
    /**
     * Search in result JSON (case-insensitive substring match)
     */
    resultContains?: string | null;
    /**
     * Filter by enforcement ID
     */
    enforcement?: number | null;
    /**
     * Filter by parent execution ID
     */
    parent?: number | null;
    /**
     * If true, only return top-level executions (those without a parent).
     * Useful for the "By Workflow" view where child tasks are loaded separately.
     */
    topLevelOnly?: boolean | null;
    /**
     * If true, include exact total counts in pagination metadata.
     * Defaults to false for the main executions list to avoid expensive count queries.
     */
    includeTotal?: boolean | null;
    /**
     * Page number (for pagination)
     */
    page?: number;
    /**
     * Items per page (for pagination)
     */
    perPage?: number;
  }): CancelablePromise<PaginatedResponse_ExecutionSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions",
      query: {
        status: status,
        action_ref: actionRef,
        pack_name: packName,
        rule_ref: ruleRef,
        trigger_ref: triggerRef,
        trace_tag: traceTag,
        executor: executor,
        result_contains: resultContains,
        enforcement: enforcement,
        parent: parent,
        top_level_only: topLevelOnly,
        include_total: includeTotal,
        page: page,
        per_page: perPage,
      },
    });
  }
  /**
   * Create a new execution (manual execution)
   * This endpoint allows directly executing an action without a trigger or rule.
   * The execution is queued and will be picked up by the executor service.
   * @returns ExecutionResponse Execution created and queued
   * @throws ApiError
   */
  public static createExecution({
    requestBody,
  }: {
    requestBody: CreateExecutionRequest;
  }): CancelablePromise<ExecutionResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/executions/execute",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        404: `Action not found`,
      },
    });
  }
  /**
   * Get execution statistics
   * @returns any Execution statistics
   * @throws ApiError
   */
  public static getExecutionStats(): CancelablePromise<Record<string, any>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/stats",
      errors: {
        500: `Internal server error`,
      },
    });
  }
  /**
   * List executions by status
   * @returns PaginatedResponse_ExecutionSummary List of executions with specified status
   * @throws ApiError
   */
  public static listExecutionsByStatus({
    status,
    page,
    pageSize,
  }: {
    /**
     * Execution status (requested, scheduling, scheduled, running, completed, failed, canceling, cancelled, timeout, abandoned)
     */
    status: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_ExecutionSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/status/{status}",
      path: {
        status: status,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        400: `Invalid status`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Create execution routes
   * Stream execution updates via Server-Sent Events
   * This endpoint streams real-time updates for execution status changes.
   * Optionally filter by execution_id to watch a specific execution.
   *
   *
   * @returns any SSE stream of execution updates
   * @throws ApiError
   */
  public static streamExecutionUpdates({
    executionId,
  }: {
    /**
     * Optional execution ID to filter updates
     */
    executionId?: number;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/stream",
      query: {
        execution_id: executionId,
      },
      errors: {
        401: `Unauthorized`,
      },
    });
  }
  /**
   * Get a single execution by ID
   * @returns any Execution details
   * @throws ApiError
   */
  public static getExecution({
    id,
  }: {
    /**
     * Execution ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Response DTO for execution information
     */
    data: {
      /**
       * Action ID (optional, may be null for ad-hoc executions)
       */
      action?: number | null;
      /**
       * Action reference
       */
      action_ref: string;
      /**
       * Retention limit override for non-log artifacts created by this execution.
       */
      artifact_retention_limit?: number | null;
      artifact_retention_policy?: null | RetentionPolicyType;
      /**
       * Execution configuration/parameters
       */
      config: Record<string, any>;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Enforcement ID (rule enforcement that triggered this)
       */
      enforcement?: number | null;
      /**
       * Identity ID that initiated this execution
       */
      executor?: number | null;
      /**
       * Execution ID
       */
      id: number;
      /**
       * ID of the original execution if this execution is a retry.
       */
      original_execution?: number | null;
      /**
       * Parent execution ID (for nested/child executions)
       */
      parent?: number | null;
      /**
       * Permission set refs embedded in the execution-scoped API token.
       */
      permission_set_refs?: Array<string>;
      /**
       * Execution result/output
       */
      result: Record<string, any>;
      /**
       * When the execution actually started running (worker picked it up).
       * Null if the execution hasn't started running yet.
       */
      started_at?: string | null;
      /**
       * Execution status
       */
      status: ExecutionStatus;
      /**
       * Resolved execution timeout in seconds, snapshotted at creation time.
       */
      timeout_seconds?: number | null;
      /**
       * System-wide trace tag for correlating related automatic activity.
       */
      trace_tag?: string | null;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Worker ID currently assigned to this execution
       */
      worker?: number | null;
      /**
       * Worker affinity override stored on the execution, if any.
       */
      worker_affinity?: any | null;
      /**
       * Worker selector override stored on the execution, if any.
       */
      worker_selector?: any | null;
      /**
       * Worker tolerations override stored on the execution, if any.
       */
      worker_tolerations?: any[] | null;
      /**
       * Workflow task metadata (only populated for workflow task executions)
       */
      workflow_task?: any | null;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Execution not found`,
      },
    });
  }
  /**
   * Cancel a running execution
   * This endpoint requests cancellation of an execution. The execution must be in a
   * cancellable state (requested, scheduling, scheduled, running, or canceling).
   * For running executions, the worker will send SIGINT to the process, then SIGTERM
   * after a 10-second grace period if it hasn't stopped.
   *
   * **Workflow cascading**: When a workflow (parent) execution is cancelled, all of
   * its incomplete child task executions are also cancelled. Children that haven't
   * reached a worker yet are set to Cancelled immediately; children that are running
   * receive a cancel MQ message so their worker can gracefully stop the process.
   * The workflow_execution record is also marked as Cancelled to prevent the
   * scheduler from dispatching any further tasks.
   * @returns any Cancellation requested
   * @throws ApiError
   */
  public static cancelExecution({
    id,
  }: {
    /**
     * Execution ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Response DTO for execution information
     */
    data: {
      /**
       * Action ID (optional, may be null for ad-hoc executions)
       */
      action?: number | null;
      /**
       * Action reference
       */
      action_ref: string;
      /**
       * Retention limit override for non-log artifacts created by this execution.
       */
      artifact_retention_limit?: number | null;
      artifact_retention_policy?: null | RetentionPolicyType;
      /**
       * Execution configuration/parameters
       */
      config: Record<string, any>;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Enforcement ID (rule enforcement that triggered this)
       */
      enforcement?: number | null;
      /**
       * Identity ID that initiated this execution
       */
      executor?: number | null;
      /**
       * Execution ID
       */
      id: number;
      /**
       * ID of the original execution if this execution is a retry.
       */
      original_execution?: number | null;
      /**
       * Parent execution ID (for nested/child executions)
       */
      parent?: number | null;
      /**
       * Permission set refs embedded in the execution-scoped API token.
       */
      permission_set_refs?: Array<string>;
      /**
       * Execution result/output
       */
      result: Record<string, any>;
      /**
       * When the execution actually started running (worker picked it up).
       * Null if the execution hasn't started running yet.
       */
      started_at?: string | null;
      /**
       * Execution status
       */
      status: ExecutionStatus;
      /**
       * Resolved execution timeout in seconds, snapshotted at creation time.
       */
      timeout_seconds?: number | null;
      /**
       * System-wide trace tag for correlating related automatic activity.
       */
      trace_tag?: string | null;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Worker ID currently assigned to this execution
       */
      worker?: number | null;
      /**
       * Worker affinity override stored on the execution, if any.
       */
      worker_affinity?: any | null;
      /**
       * Worker selector override stored on the execution, if any.
       */
      worker_selector?: any | null;
      /**
       * Worker tolerations override stored on the execution, if any.
       */
      worker_tolerations?: any[] | null;
      /**
       * Workflow task metadata (only populated for workflow task executions)
       */
      workflow_task?: any | null;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/executions/{id}/cancel",
      path: {
        id: id,
      },
      errors: {
        404: `Execution not found`,
        409: `Execution is not in a cancellable state`,
      },
    });
  }
  /**
   * Stream stdout/stderr for an execution as SSE.
   * This tails the worker's live log files directly from the shared artifacts
   * volume. The file may not exist yet when the worker has not emitted any
   * output, so the stream waits briefly for it to appear.
   * @returns any SSE stream of execution log content
   * @throws ApiError
   */
  public static streamExecutionLog({
    id,
    stream,
    offset,
  }: {
    /**
     * Execution ID
     */
    id: number;
    /**
     * Log stream name: stdout or stderr
     */
    stream: string;
    /**
     * Resume streaming from this byte offset
     */
    offset?: number;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/{id}/logs/{stream}/stream",
      path: {
        id: id,
        stream: stream,
      },
      query: {
        offset: offset,
      },
      errors: {
        401: `Unauthorized`,
        404: `Execution not found`,
      },
    });
  }
  /**
   * Republish a Requested execution's scheduler message.
   * This is a recovery control for executions that are still `requested` after
   * their original `ExecutionRequested` message may have been consumed during a
   * transient scheduler failure. It does not restart running work.
   * @returns any Execution request republished
   * @throws ApiError
   */
  public static rescheduleExecution({
    id,
  }: {
    /**
     * Execution ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Response DTO for manual execution reschedule requests.
     */
    data: {
      /**
       * Number of reschedule attempts recorded for this execution.
       */
      attempt_count: number;
      /**
       * Current execution row after republish.
       */
      execution: ExecutionResponse;
      /**
       * Timestamp for the recorded reschedule attempt.
       */
      last_attempt_at: string;
      /**
       * Human-readable status of the republish request.
       */
      message: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/executions/{id}/reschedule",
      path: {
        id: id,
      },
      errors: {
        404: `Execution not found`,
        409: `Execution is not eligible for reschedule`,
      },
    });
  }
  /**
   * List safe workflow cache iteration status for an execution.
   * @returns any Workflow cache iteration status
   * @throws ApiError
   */
  public static listWorkflowCacheIterations({
    id,
  }: {
    /**
     * Execution ID
     */
    id: number;
  }): CancelablePromise<{
    data: Array<{
      batch_size: number;
      completed_at?: string | null;
      concurrency: number;
      created: string;
      dispatched_count: number;
      error_summary?: string | null;
      generation_id: number;
      namespace_id: number;
      page_size: number;
      scanned_count: number;
      state: WorkflowCacheIterationState;
      task_name: string;
      updated: string;
    }>;
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/{id}/workflow-cache-iterations",
      path: {
        id: id,
      },
      errors: {
        401: `Unauthorized`,
        403: `Execution is not visible to the caller`,
        404: `Execution not found`,
      },
    });
  }
}
