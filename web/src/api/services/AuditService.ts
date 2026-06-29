/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_AuditEventResponse } from "../models/ApiResponse_AuditEventResponse";
import type { ApiResponse_Vec_AuditEventResponse } from "../models/ApiResponse_Vec_AuditEventResponse";
import type { AuditCategory } from "../models/AuditCategory";
import type { AuditOutcome } from "../models/AuditOutcome";
import type { i64 } from "../models/i64";
import type { PaginatedResponse_AuditEventSummary } from "../models/PaginatedResponse_AuditEventSummary";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class AuditService {
  /**
   * List audit events with optional filters.
   * @returns PaginatedResponse_AuditEventSummary Audit event list
   * @throws ApiError
   */
  public static listAuditEvents({
    category,
    eventType,
    outcome,
    actorIdentity,
    actorLogin,
    resourceType,
    resourceId,
    resourceRef,
    requestId,
    httpStatus,
    httpMethod,
    httpPath,
    createdAfter,
    createdBefore,
    includeTotal,
    page,
    perPage,
  }: {
    /**
     * Top-level category (`api`, `auth`, `rbac`, `secret`, `admin`,
     * `execution`, `pack`).
     */
    category?: null | AuditCategory;
    /**
     * Exact match on the dotted event-type identifier.
     */
    eventType?: string | null;
    /**
     * Outcome (`success`, `failure`, `denied`).
     */
    outcome?: null | AuditOutcome;
    /**
     * Filter by actor identity ID.
     */
    actorIdentity?: null | i64;
    /**
     * Substring match (case-insensitive) on actor login.
     */
    actorLogin?: string | null;
    /**
     * Logical resource type (`pack`, `key`, `action`, …).
     */
    resourceType?: string | null;
    /**
     * Filter by resource ID.
     */
    resourceId?: null | i64;
    /**
     * Exact match on resource ref.
     */
    resourceRef?: string | null;
    /**
     * Filter by request_id correlation UUID.
     */
    requestId?: string | null;
    /**
     * HTTP status code (typed-API events only).
     */
    httpStatus?: number | null;
    /**
     * HTTP method (`GET`, `POST`, …).
     */
    httpMethod?: string | null;
    /**
     * Substring match on the HTTP path.
     */
    httpPath?: string | null;
    /**
     * Lower bound on `created` (inclusive, RFC3339).
     */
    createdAfter?: string | null;
    /**
     * Upper bound on `created` (exclusive, RFC3339).
     */
    createdBefore?: string | null;
    /**
     * Include exact total count in pagination metadata.
     */
    includeTotal?: boolean | null;
    /**
     * Page number (1-indexed).
     */
    page?: number;
    /**
     * Items per page.
     */
    perPage?: number;
  }): CancelablePromise<PaginatedResponse_AuditEventSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/audit-events",
      query: {
        category: category,
        event_type: eventType,
        outcome: outcome,
        actor_identity: actorIdentity,
        actor_login: actorLogin,
        resource_type: resourceType,
        resource_id: resourceId,
        resource_ref: resourceRef,
        request_id: requestId,
        http_status: httpStatus,
        http_method: httpMethod,
        http_path: httpPath,
        created_after: createdAfter,
        created_before: createdBefore,
        include_total: includeTotal,
        page: page,
        per_page: perPage,
      },
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Get all audit events sharing a request_id (full request lineage).
   * @returns ApiResponse_Vec_AuditEventResponse Audit events for the request
   * @throws ApiError
   */
  public static getAuditEventsByRequest({
    requestId,
  }: {
    /**
     * Correlation UUID
     */
    requestId: string;
  }): CancelablePromise<ApiResponse_Vec_AuditEventResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/audit-events/by-request/{request_id}",
      path: {
        request_id: requestId,
      },
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Get a single audit event by ID.
   * @returns ApiResponse_AuditEventResponse Audit event details
   * @throws ApiError
   */
  public static getAuditEvent({
    id,
  }: {
    /**
     * Audit event ID
     */
    id: number;
  }): CancelablePromise<ApiResponse_AuditEventResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/audit-events/{id}",
      path: {
        id: id,
      },
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Audit event not found`,
        500: `Internal server error`,
      },
    });
  }
}
