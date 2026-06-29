/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_EnforcementResponse } from "../models/ApiResponse_EnforcementResponse";
import type { EnforcementStatus } from "../models/EnforcementStatus";
import type { i64 } from "../models/i64";
import type { PaginatedResponse_EnforcementSummary } from "../models/PaginatedResponse_EnforcementSummary";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class EnforcementsService {
  /**
   * List all enforcements with pagination and optional filters
   * @returns PaginatedResponse_EnforcementSummary List of enforcements
   * @throws ApiError
   */
  public static listEnforcements({
    rule,
    event,
    status,
    triggerRef,
    ruleRef,
    traceTag,
    includeTotal,
    page,
    perPage,
  }: {
    /**
     * Filter by rule ID
     */
    rule?: null | i64;
    /**
     * Filter by event ID
     */
    event?: null | i64;
    /**
     * Filter by status
     */
    status?: null | EnforcementStatus;
    /**
     * Filter by trigger reference
     */
    triggerRef?: string | null;
    /**
     * Filter by rule reference
     */
    ruleRef?: string | null;
    /**
     * Filter by exact trace tag on associated executions.
     */
    traceTag?: string | null;
    /**
     * If true, include exact total counts in pagination metadata.
     */
    includeTotal?: boolean | null;
    /**
     * Page number (1-indexed)
     */
    page?: number;
    /**
     * Items per page
     */
    perPage?: number;
  }): CancelablePromise<PaginatedResponse_EnforcementSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/enforcements",
      query: {
        rule: rule,
        event: event,
        status: status,
        trigger_ref: triggerRef,
        rule_ref: ruleRef,
        trace_tag: traceTag,
        include_total: includeTotal,
        page: page,
        per_page: perPage,
      },
      errors: {
        401: `Unauthorized`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Get a single enforcement by ID
   * @returns ApiResponse_EnforcementResponse Enforcement details
   * @throws ApiError
   */
  public static getEnforcement({
    id,
  }: {
    /**
     * Enforcement ID
     */
    id: number;
  }): CancelablePromise<ApiResponse_EnforcementResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/enforcements/{id}",
      path: {
        id: id,
      },
      errors: {
        401: `Unauthorized`,
        404: `Enforcement not found`,
        500: `Internal server error`,
      },
    });
  }
}
