/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_RuleResponse } from '../models/ApiResponse_RuleResponse';
import type { CreateRuleRequest } from '../models/CreateRuleRequest';
import type { PaginatedResponse_RuleSummary } from '../models/PaginatedResponse_RuleSummary';
import type { SuccessResponse } from '../models/SuccessResponse';
import type { UpdateRuleRequest } from '../models/UpdateRuleRequest';
import type { CancelablePromise } from '../core/CancelablePromise';
import { OpenAPI } from '../core/OpenAPI';
import { request as __request } from '../core/request';
export class RulesService {
    /**
     * List rules by action reference
     * @returns PaginatedResponse_RuleSummary List of rules using this action
     * @throws ApiError
     */
    public static listRulesByAction({
        actionRef,
        page,
        pageSize,
    }: {
        /**
         * Action reference
         */
        actionRef: string,
        /**
         * Page number (1-based)
         */
        page?: number,
        /**
         * Number of items per page
         */
        pageSize?: number,
    }): CancelablePromise<PaginatedResponse_RuleSummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/actions/{action_ref}/rules',
            path: {
                'action_ref': actionRef,
            },
            query: {
                'page': page,
                'page_size': pageSize,
            },
            errors: {
                404: `Action not found`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * List rules by pack reference
     * @returns PaginatedResponse_RuleSummary List of rules in pack
     * @throws ApiError
     */
    public static listRulesByPack({
        packRef,
        page,
        pageSize,
    }: {
        /**
         * Pack reference
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
    }): CancelablePromise<PaginatedResponse_RuleSummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/packs/{pack_ref}/rules',
            path: {
                'pack_ref': packRef,
            },
            query: {
                'page': page,
                'page_size': pageSize,
            },
            errors: {
                404: `Pack not found`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * List all rules with pagination
     * @returns PaginatedResponse_RuleSummary List of rules
     * @throws ApiError
     */
    public static listRules({
        page,
        pageSize,
        packRef,
        actionRef,
        triggerRef,
        enabled,
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
         * Optional pack ref filter
         */
        packRef?: string | null,
        /**
         * Optional action ref filter
         */
        actionRef?: string | null,
        /**
         * Optional trigger ref filter
         */
        triggerRef?: string | null,
        /**
         * Optional enabled-state filter
         */
        enabled?: boolean | null,
    }): CancelablePromise<PaginatedResponse_RuleSummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/rules',
            query: {
                'page': page,
                'page_size': pageSize,
                'pack_ref': packRef,
                'action_ref': actionRef,
                'trigger_ref': triggerRef,
                'enabled': enabled,
            },
            errors: {
                500: `Internal server error`,
            },
        });
    }
    /**
     * Create a new rule
     * @returns ApiResponse_RuleResponse Rule created successfully
     * @throws ApiError
     */
    public static createRule({
        requestBody,
    }: {
        requestBody: CreateRuleRequest,
    }): CancelablePromise<ApiResponse_RuleResponse> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/rules',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Invalid request`,
                404: `Pack, action, or trigger not found`,
                409: `Rule with same ref already exists`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * List enabled rules
     * @returns PaginatedResponse_RuleSummary List of enabled rules
     * @throws ApiError
     */
    public static listEnabledRules({
        page,
        pageSize,
    }: {
        /**
         * Page number (1-based)
         */
        page?: number,
        /**
         * Number of items per page
         */
        pageSize?: number,
    }): CancelablePromise<PaginatedResponse_RuleSummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/rules/enabled',
            query: {
                'page': page,
                'page_size': pageSize,
            },
            errors: {
                500: `Internal server error`,
            },
        });
    }
    /**
     * Get a single rule by reference
     * @returns ApiResponse_RuleResponse Rule details
     * @throws ApiError
     */
    public static getRule({
        ref,
    }: {
        /**
         * Rule reference
         */
        ref: string,
    }): CancelablePromise<ApiResponse_RuleResponse> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/rules/{ref}',
            path: {
                'ref': ref,
            },
            errors: {
                404: `Rule not found`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * Update an existing rule
     * @returns ApiResponse_RuleResponse Rule updated successfully
     * @throws ApiError
     */
    public static updateRule({
        ref,
        requestBody,
    }: {
        /**
         * Rule reference
         */
        ref: string,
        requestBody: UpdateRuleRequest,
    }): CancelablePromise<ApiResponse_RuleResponse> {
        return __request(OpenAPI, {
            method: 'PUT',
            url: '/api/v1/rules/{ref}',
            path: {
                'ref': ref,
            },
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Invalid request`,
                404: `Rule not found`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * Delete a rule
     * @returns SuccessResponse Rule deleted successfully
     * @throws ApiError
     */
    public static deleteRule({
        ref,
    }: {
        /**
         * Rule reference
         */
        ref: string,
    }): CancelablePromise<SuccessResponse> {
        return __request(OpenAPI, {
            method: 'DELETE',
            url: '/api/v1/rules/{ref}',
            path: {
                'ref': ref,
            },
            errors: {
                404: `Rule not found`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * Disable a rule
     * @returns ApiResponse_RuleResponse Rule disabled successfully
     * @throws ApiError
     */
    public static disableRule({
        ref,
    }: {
        /**
         * Rule reference
         */
        ref: string,
    }): CancelablePromise<ApiResponse_RuleResponse> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/rules/{ref}/disable',
            path: {
                'ref': ref,
            },
            errors: {
                404: `Rule not found`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * Enable a rule
     * @returns ApiResponse_RuleResponse Rule enabled successfully
     * @throws ApiError
     */
    public static enableRule({
        ref,
    }: {
        /**
         * Rule reference
         */
        ref: string,
    }): CancelablePromise<ApiResponse_RuleResponse> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/rules/{ref}/enable',
            path: {
                'ref': ref,
            },
            errors: {
                404: `Rule not found`,
                500: `Internal server error`,
            },
        });
    }
    /**
     * List rules by trigger reference
     * @returns PaginatedResponse_RuleSummary List of rules using this trigger
     * @throws ApiError
     */
    public static listRulesByTrigger({
        triggerRef,
        page,
        pageSize,
    }: {
        /**
         * Trigger reference
         */
        triggerRef: string,
        /**
         * Page number (1-based)
         */
        page?: number,
        /**
         * Number of items per page
         */
        pageSize?: number,
    }): CancelablePromise<PaginatedResponse_RuleSummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/triggers/{trigger_ref}/rules',
            path: {
                'trigger_ref': triggerRef,
            },
            query: {
                'page': page,
                'page_size': pageSize,
            },
            errors: {
                404: `Trigger not found`,
                500: `Internal server error`,
            },
        });
    }
}
