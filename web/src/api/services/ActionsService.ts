/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from '../models/ActionReferenceVisibility';
import type { CreateActionRequest } from '../models/CreateActionRequest';
import type { PaginatedResponse_ActionSearchHit } from '../models/PaginatedResponse_ActionSearchHit';
import type { PaginatedResponse_ActionSummary } from '../models/PaginatedResponse_ActionSummary';
import type { RetentionPolicyType } from '../models/RetentionPolicyType';
import type { SuccessResponse } from '../models/SuccessResponse';
import type { UpdateActionRequest } from '../models/UpdateActionRequest';
import type { WorkerAffinity } from '../models/WorkerAffinity';
import type { WorkerToleration } from '../models/WorkerToleration';
import type { CancelablePromise } from '../core/CancelablePromise';
import { OpenAPI } from '../core/OpenAPI';
import { request as __request } from '../core/request';
export class ActionsService {
    /**
     * List all actions with pagination
     * @returns PaginatedResponse_ActionSummary List of actions
     * @throws ApiError
     */
    public static listActions({
        page,
        pageSize,
        executableWithCurrentAccess,
        referencingPackRef,
    }: {
        /**
         * Page number (1-based)
         */
        page?: number,
        /**
         * Number of items per page
         */
        pageSize?: number,
        /**
         * When true, only return actions the current token can execute and whose
         * default execution permission sets can be delegated by the current token.
         */
        executableWithCurrentAccess?: boolean,
        /**
         * Optional pack ref that wants to reference the returned actions.
         */
        referencingPackRef?: string | null,
    }): CancelablePromise<PaginatedResponse_ActionSummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/actions',
            query: {
                'page': page,
                'page_size': pageSize,
                'executable_with_current_access': executableWithCurrentAccess,
                'referencing_pack_ref': referencingPackRef,
            },
        });
    }
    /**
     * Create a new action
     * @returns any Action created successfully
     * @throws ApiError
     */
    public static createAction({
        requestBody,
    }: {
        requestBody: CreateActionRequest,
    }): CancelablePromise<{
        /**
         * Response DTO for action information
         */
        data: {
            /**
             * Hint that this action may invoke the Attune MCP server and spawn child executions.
             */
            accesses_mcp: boolean;
            /**
             * Per-action retention limit override for non-log artifacts created by executions.
             */
            artifact_retention_limit?: number | null;
            artifact_retention_policy?: (null | RetentionPolicyType);
            /**
             * Creation timestamp
             */
            created: string;
            /**
             * Default permission set refs used when executions do not explicitly override token permissions.
             */
            default_execution_permission_set_refs?: Array<string>;
            /**
             * Action description
             */
            description?: string | null;
            /**
             * Whether this action is enabled
             */
            enabled: boolean;
            /**
             * Entry point
             */
            entrypoint: string;
            /**
             * Action ID
             */
            id: number;
            /**
             * Whether this is an ad-hoc action (not from pack installation)
             */
            is_adhoc: boolean;
            /**
             * Human-readable label
             */
            label: string;
            /**
             * Per-action retention limit override for stdout/stderr execution log artifacts.
             */
            log_retention_limit?: number | null;
            log_retention_policy?: (null | RetentionPolicyType);
            /**
             * Output schema
             */
            out_schema: any | null;
            /**
             * Pack ID
             */
            pack: number;
            /**
             * Pack reference
             */
            pack_ref: string;
            /**
             * Parameter schema (StackStorm-style with inline required/secret)
             */
            param_schema: any | null;
            /**
             * Unique reference identifier
             */
            ref: string;
            /**
             * Pack refs allowed to reference this action when visibility is restricted.
             */
            reference_allowed_pack_refs?: Array<string>;
            /**
             * Pack-level visibility for references from rules, workflows, and queues.
             */
            reference_visibility: ActionReferenceVisibility;
            /**
             * Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
             */
            required_worker_runtimes?: Record<string, any>;
            /**
             * Runtime ID
             */
            runtime?: number | null;
            /**
             * Runtime reference (stable identifier, e.g., "core.python")
             */
            runtime_ref?: string | null;
            /**
             * Semver version constraint for the runtime (e.g., ">=3.12", ">=3.12,<4.0", "~18.0")
             */
            runtime_version_constraint?: string | null;
            /**
             * Default execution timeout (seconds) snapshotted onto executions of this action.
             */
            timeout_seconds?: number | null;
            /**
             * Last update timestamp
             */
            updated: string;
            /**
             * Required/preferred worker label affinity and required anti-affinity.
             */
            worker_affinity?: WorkerAffinity;
            /**
             * Exact worker label requirements.
             */
            worker_selector?: Record<string, any>;
            /**
             * Tolerations for worker taints.
             */
            worker_tolerations?: Array<WorkerToleration>;
            /**
             * Workflow definition ID (non-null if this action is a workflow)
             */
            workflow_def?: number | null;
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/actions',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                404: `Pack not found`,
                409: `Action with same ref already exists`,
            },
        });
    }
    /**
     * Search for actions by keyword and pack filter.
     * Returns lean `ActionSearchHit` rows optimized for action discovery — useful
     * for AI agents and human browsing of large action catalogs. Whitespace-separated
     * tokens in `q` are AND-matched (each token must appear in at least one of
     * `ref`, `label`, `description`, or `pack_ref`).
     * @returns PaginatedResponse_ActionSearchHit Matching actions
     * @throws ApiError
     */
    public static searchActions({
        q,
        packs,
        referencingPackRef,
        page,
        pageSize,
    }: {
        /**
         * Keyword query. Whitespace-separated tokens are AND-matched against
         * `ref`, `label`, `description`, and `pack_ref` (case-insensitive substring).
         */
        q?: string | null,
        /**
         * Restrict to one or more pack refs. Comma-separated (e.g., `core,slack,jira`)
         * or repeated query params (e.g., `?packs=core&packs=slack`).
         */
        packs?: string | null,
        /**
         * Optional pack ref that wants to reference the returned actions.
         * When set, restricted actions allow-listed for this pack are included.
         */
        referencingPackRef?: string | null,
        /**
         * Page number (1-based)
         */
        page?: number,
        /**
         * Number of items per page
         */
        pageSize?: number,
    }): CancelablePromise<PaginatedResponse_ActionSearchHit> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/actions/search',
            query: {
                'q': q,
                'packs': packs,
                'referencing_pack_ref': referencingPackRef,
                'page': page,
                'page_size': pageSize,
            },
            errors: {
                404: `One or more pack refs not found`,
            },
        });
    }
    /**
     * Get a single action by reference
     * @returns any Action details
     * @throws ApiError
     */
    public static getAction({
        ref,
    }: {
        /**
         * Action reference identifier
         */
        ref: string,
    }): CancelablePromise<{
        /**
         * Response DTO for action information
         */
        data: {
            /**
             * Hint that this action may invoke the Attune MCP server and spawn child executions.
             */
            accesses_mcp: boolean;
            /**
             * Per-action retention limit override for non-log artifacts created by executions.
             */
            artifact_retention_limit?: number | null;
            artifact_retention_policy?: (null | RetentionPolicyType);
            /**
             * Creation timestamp
             */
            created: string;
            /**
             * Default permission set refs used when executions do not explicitly override token permissions.
             */
            default_execution_permission_set_refs?: Array<string>;
            /**
             * Action description
             */
            description?: string | null;
            /**
             * Whether this action is enabled
             */
            enabled: boolean;
            /**
             * Entry point
             */
            entrypoint: string;
            /**
             * Action ID
             */
            id: number;
            /**
             * Whether this is an ad-hoc action (not from pack installation)
             */
            is_adhoc: boolean;
            /**
             * Human-readable label
             */
            label: string;
            /**
             * Per-action retention limit override for stdout/stderr execution log artifacts.
             */
            log_retention_limit?: number | null;
            log_retention_policy?: (null | RetentionPolicyType);
            /**
             * Output schema
             */
            out_schema: any | null;
            /**
             * Pack ID
             */
            pack: number;
            /**
             * Pack reference
             */
            pack_ref: string;
            /**
             * Parameter schema (StackStorm-style with inline required/secret)
             */
            param_schema: any | null;
            /**
             * Unique reference identifier
             */
            ref: string;
            /**
             * Pack refs allowed to reference this action when visibility is restricted.
             */
            reference_allowed_pack_refs?: Array<string>;
            /**
             * Pack-level visibility for references from rules, workflows, and queues.
             */
            reference_visibility: ActionReferenceVisibility;
            /**
             * Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
             */
            required_worker_runtimes?: Record<string, any>;
            /**
             * Runtime ID
             */
            runtime?: number | null;
            /**
             * Runtime reference (stable identifier, e.g., "core.python")
             */
            runtime_ref?: string | null;
            /**
             * Semver version constraint for the runtime (e.g., ">=3.12", ">=3.12,<4.0", "~18.0")
             */
            runtime_version_constraint?: string | null;
            /**
             * Default execution timeout (seconds) snapshotted onto executions of this action.
             */
            timeout_seconds?: number | null;
            /**
             * Last update timestamp
             */
            updated: string;
            /**
             * Required/preferred worker label affinity and required anti-affinity.
             */
            worker_affinity?: WorkerAffinity;
            /**
             * Exact worker label requirements.
             */
            worker_selector?: Record<string, any>;
            /**
             * Tolerations for worker taints.
             */
            worker_tolerations?: Array<WorkerToleration>;
            /**
             * Workflow definition ID (non-null if this action is a workflow)
             */
            workflow_def?: number | null;
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/actions/{ref}',
            path: {
                'ref': ref,
            },
            errors: {
                404: `Action not found`,
            },
        });
    }
    /**
     * Update an existing action
     * @returns any Action updated successfully
     * @throws ApiError
     */
    public static updateAction({
        ref,
        requestBody,
    }: {
        /**
         * Action reference identifier
         */
        ref: string,
        requestBody: UpdateActionRequest,
    }): CancelablePromise<{
        /**
         * Response DTO for action information
         */
        data: {
            /**
             * Hint that this action may invoke the Attune MCP server and spawn child executions.
             */
            accesses_mcp: boolean;
            /**
             * Per-action retention limit override for non-log artifacts created by executions.
             */
            artifact_retention_limit?: number | null;
            artifact_retention_policy?: (null | RetentionPolicyType);
            /**
             * Creation timestamp
             */
            created: string;
            /**
             * Default permission set refs used when executions do not explicitly override token permissions.
             */
            default_execution_permission_set_refs?: Array<string>;
            /**
             * Action description
             */
            description?: string | null;
            /**
             * Whether this action is enabled
             */
            enabled: boolean;
            /**
             * Entry point
             */
            entrypoint: string;
            /**
             * Action ID
             */
            id: number;
            /**
             * Whether this is an ad-hoc action (not from pack installation)
             */
            is_adhoc: boolean;
            /**
             * Human-readable label
             */
            label: string;
            /**
             * Per-action retention limit override for stdout/stderr execution log artifacts.
             */
            log_retention_limit?: number | null;
            log_retention_policy?: (null | RetentionPolicyType);
            /**
             * Output schema
             */
            out_schema: any | null;
            /**
             * Pack ID
             */
            pack: number;
            /**
             * Pack reference
             */
            pack_ref: string;
            /**
             * Parameter schema (StackStorm-style with inline required/secret)
             */
            param_schema: any | null;
            /**
             * Unique reference identifier
             */
            ref: string;
            /**
             * Pack refs allowed to reference this action when visibility is restricted.
             */
            reference_allowed_pack_refs?: Array<string>;
            /**
             * Pack-level visibility for references from rules, workflows, and queues.
             */
            reference_visibility: ActionReferenceVisibility;
            /**
             * Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
             */
            required_worker_runtimes?: Record<string, any>;
            /**
             * Runtime ID
             */
            runtime?: number | null;
            /**
             * Runtime reference (stable identifier, e.g., "core.python")
             */
            runtime_ref?: string | null;
            /**
             * Semver version constraint for the runtime (e.g., ">=3.12", ">=3.12,<4.0", "~18.0")
             */
            runtime_version_constraint?: string | null;
            /**
             * Default execution timeout (seconds) snapshotted onto executions of this action.
             */
            timeout_seconds?: number | null;
            /**
             * Last update timestamp
             */
            updated: string;
            /**
             * Required/preferred worker label affinity and required anti-affinity.
             */
            worker_affinity?: WorkerAffinity;
            /**
             * Exact worker label requirements.
             */
            worker_selector?: Record<string, any>;
            /**
             * Tolerations for worker taints.
             */
            worker_tolerations?: Array<WorkerToleration>;
            /**
             * Workflow definition ID (non-null if this action is a workflow)
             */
            workflow_def?: number | null;
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'PUT',
            url: '/api/v1/actions/{ref}',
            path: {
                'ref': ref,
            },
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                404: `Action not found`,
            },
        });
    }
    /**
     * Delete an action
     * @returns SuccessResponse Action deleted successfully
     * @throws ApiError
     */
    public static deleteAction({
        ref,
    }: {
        /**
         * Action reference identifier
         */
        ref: string,
    }): CancelablePromise<SuccessResponse> {
        return __request(OpenAPI, {
            method: 'DELETE',
            url: '/api/v1/actions/{ref}',
            path: {
                'ref': ref,
            },
            errors: {
                404: `Action not found`,
            },
        });
    }
    /**
     * Get queue statistics for an action
     * @returns any Queue statistics
     * @throws ApiError
     */
    public static getQueueStats({
        ref,
    }: {
        /**
         * Action reference identifier
         */
        ref: string,
    }): CancelablePromise<{
        /**
         * Response DTO for queue statistics
         */
        data: {
            /**
             * Action ID
             */
            action_id: number;
            /**
             * Action reference
             */
            action_ref: string;
            /**
             * Number of currently running executions
             */
            active_count: number;
            /**
             * Timestamp of last statistics update
             */
            last_updated: string;
            /**
             * Maximum concurrent executions allowed
             */
            max_concurrent: number;
            /**
             * Timestamp of oldest queued execution (if any)
             */
            oldest_enqueued_at?: string | null;
            /**
             * Number of executions waiting in queue
             */
            queue_length: number;
            /**
             * Total executions completed since queue creation
             */
            total_completed: number;
            /**
             * Total executions enqueued since queue creation
             */
            total_enqueued: number;
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/actions/{ref}/queue-stats',
            path: {
                'ref': ref,
            },
            errors: {
                404: `Action not found or no queue statistics available`,
            },
        });
    }
    /**
     * List actions by pack reference
     * @returns PaginatedResponse_ActionSummary List of actions for pack
     * @throws ApiError
     */
    public static listActionsByPack({
        packRef,
        page,
        pageSize,
    }: {
        /**
         * Pack reference identifier
         */
        packRef: string,
        /**
         * Page number (1-based)
         */
        page?: number,
        /**
         * Number of items per page
         */
        pageSize?: number,
    }): CancelablePromise<PaginatedResponse_ActionSummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/packs/{pack_ref}/actions',
            path: {
                'pack_ref': packRef,
            },
            query: {
                'page': page,
                'page_size': pageSize,
            },
            errors: {
                404: `Pack not found`,
            },
        });
    }
}
