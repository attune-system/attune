/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_InquiryResponse } from "../models/ApiResponse_InquiryResponse";
import type { CreateInquiryRequest } from "../models/CreateInquiryRequest";
import type { i64 } from "../models/i64";
import type { InquiryRespondRequest } from "../models/InquiryRespondRequest";
import type { InquiryStatus } from "../models/InquiryStatus";
import type { PaginatedResponse_InquirySummary } from "../models/PaginatedResponse_InquirySummary";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateInquiryRequest } from "../models/UpdateInquiryRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class InquiriesService {
  /**
   * List inquiries for a specific execution
   * @returns PaginatedResponse_InquirySummary List of inquiries for execution
   * @throws ApiError
   */
  public static listInquiriesByExecution({
    executionId,
    page,
    pageSize,
  }: {
    /**
     * Execution ID
     */
    executionId: number;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_InquirySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/{execution_id}/inquiries",
      path: {
        execution_id: executionId,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        401: `Unauthorized`,
        404: `Execution not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * List all inquiries with pagination and optional filters
   * @returns PaginatedResponse_InquirySummary List of inquiries
   * @throws ApiError
   */
  public static listInquiries({
    status,
    execution,
    assignedTo,
    offset,
    limit,
  }: {
    /**
     * Filter by status
     */
    status?: null | InquiryStatus;
    /**
     * Filter by execution ID
     */
    execution?: null | i64;
    /**
     * Filter by assigned identity
     */
    assignedTo?: null | i64;
    /**
     * Pagination offset
     */
    offset?: number | null;
    /**
     * Pagination limit
     */
    limit?: number | null;
  }): CancelablePromise<PaginatedResponse_InquirySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/inquiries",
      query: {
        status: status,
        execution: execution,
        assigned_to: assignedTo,
        offset: offset,
        limit: limit,
      },
      errors: {
        401: `Unauthorized`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Create a new inquiry
   * @returns ApiResponse_InquiryResponse Inquiry created successfully
   * @throws ApiError
   */
  public static createInquiry({
    requestBody,
  }: {
    requestBody: CreateInquiryRequest;
  }): CancelablePromise<ApiResponse_InquiryResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/inquiries",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        401: `Unauthorized`,
        404: `Execution not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * List inquiries by status
   * @returns PaginatedResponse_InquirySummary List of inquiries with specified status
   * @throws ApiError
   */
  public static listInquiriesByStatus({
    status,
    page,
    pageSize,
  }: {
    /**
     * Inquiry status (pending, responded, timeout, canceled)
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
  }): CancelablePromise<PaginatedResponse_InquirySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/inquiries/status/{status}",
      path: {
        status: status,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        400: `Invalid status`,
        401: `Unauthorized`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Get a single inquiry by ID
   * @returns ApiResponse_InquiryResponse Inquiry details
   * @throws ApiError
   */
  public static getInquiry({
    id,
  }: {
    /**
     * Inquiry ID
     */
    id: number;
  }): CancelablePromise<ApiResponse_InquiryResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/inquiries/{id}",
      path: {
        id: id,
      },
      errors: {
        401: `Unauthorized`,
        404: `Inquiry not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Update an existing inquiry
   * @returns ApiResponse_InquiryResponse Inquiry updated successfully
   * @throws ApiError
   */
  public static updateInquiry({
    id,
    requestBody,
  }: {
    /**
     * Inquiry ID
     */
    id: number;
    requestBody: UpdateInquiryRequest;
  }): CancelablePromise<ApiResponse_InquiryResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/inquiries/{id}",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        401: `Unauthorized`,
        404: `Inquiry not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Delete an inquiry
   * @returns SuccessResponse Inquiry deleted successfully
   * @throws ApiError
   */
  public static deleteInquiry({
    id,
  }: {
    /**
     * Inquiry ID
     */
    id: number;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/inquiries/{id}",
      path: {
        id: id,
      },
      errors: {
        401: `Unauthorized`,
        404: `Inquiry not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Respond to an inquiry (user-facing endpoint)
   * @returns ApiResponse_InquiryResponse Response submitted successfully
   * @throws ApiError
   */
  public static respondToInquiry({
    id,
    requestBody,
  }: {
    /**
     * Inquiry ID
     */
    id: number;
    requestBody: InquiryRespondRequest;
  }): CancelablePromise<ApiResponse_InquiryResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/inquiries/{id}/respond",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request or inquiry cannot be responded to`,
        401: `Unauthorized`,
        403: `Not authorized to respond to this inquiry`,
        404: `Inquiry not found`,
        500: `Internal server error`,
      },
    });
  }
}
