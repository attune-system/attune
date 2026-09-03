/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_EventResponse } from "../models/ApiResponse_EventResponse";
import type { CreateEventRequest } from "../models/CreateEventRequest";
import type { i64 } from "../models/i64";
import type { PaginatedResponse_EventSummary } from "../models/PaginatedResponse_EventSummary";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class EventsService {
  /**
   * List all events with pagination and optional filters
   * @returns PaginatedResponse_EventSummary List of events
   * @throws ApiError
   */
  public static listEvents({
    trigger,
    triggerRef,
    ruleRef,
    source,
    traceTag,
    includeTotal,
    page,
    perPage,
  }: {
    /**
     * Filter by trigger ID
     */
    trigger?: null | i64;
    /**
     * Filter by trigger reference
     */
    triggerRef?: string | null;
    /**
     * Filter by rule reference
     */
    ruleRef?: string | null;
    /**
     * Filter by source ID
     */
    source?: null | i64;
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
  }): CancelablePromise<PaginatedResponse_EventSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/events",
      query: {
        trigger: trigger,
        trigger_ref: triggerRef,
        rule_ref: ruleRef,
        source: source,
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
   * Create a new event
   * @returns ApiResponse_EventResponse Event created successfully
   * @throws ApiError
   */
  public static createEvent({
    requestBody,
  }: {
    requestBody: CreateEventRequest;
  }): CancelablePromise<ApiResponse_EventResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/events",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        401: `Unauthorized`,
        403: `Token is not authorized to create this event`,
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Get a single event by ID
   * @returns ApiResponse_EventResponse Event details
   * @throws ApiError
   */
  public static getEvent({
    id,
    includeSecretValues,
  }: {
    /**
     * Event ID
     */
    id: number;
    /**
     * Include decrypted secret payload/config values. Requires events:decrypt.
     */
    includeSecretValues?: boolean;
  }): CancelablePromise<ApiResponse_EventResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/events/{id}",
      path: {
        id: id,
      },
      query: {
        include_secret_values: includeSecretValues,
      },
      errors: {
        401: `Unauthorized`,
        404: `Event not found`,
        500: `Internal server error`,
      },
    });
  }
}
