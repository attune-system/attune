/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_RuntimeResponse } from "../models/ApiResponse_RuntimeResponse";
import type { CreateRuntimeRequest } from "../models/CreateRuntimeRequest";
import type { PaginatedResponse_RuntimeSummary } from "../models/PaginatedResponse_RuntimeSummary";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateRuntimeRequest } from "../models/UpdateRuntimeRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class RuntimesService {
  /**
   * @returns PaginatedResponse_RuntimeSummary List of runtimes for a pack
   * @throws ApiError
   */
  public static listRuntimesByPack({
    packRef,
    page,
    pageSize,
  }: {
    /**
     * Pack reference identifier
     */
    packRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_RuntimeSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/runtimes",
      path: {
        pack_ref: packRef,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        404: `Pack not found`,
      },
    });
  }
  /**
   * @returns PaginatedResponse_RuntimeSummary List of runtimes
   * @throws ApiError
   */
  public static listRuntimes({
    page,
    pageSize,
    q,
  }: {
    page?: number;
    pageSize?: number;
    /**
     * Keyword query. Tokens are AND-matched across ref, name, description,
     * and pack_ref.
     */
    q?: string | null;
  }): CancelablePromise<PaginatedResponse_RuntimeSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/runtimes",
      query: {
        page: page,
        page_size: pageSize,
        q: q,
      },
    });
  }
  /**
   * @returns ApiResponse_RuntimeResponse Runtime created successfully
   * @throws ApiError
   */
  public static createRuntime({
    requestBody,
  }: {
    requestBody: CreateRuntimeRequest;
  }): CancelablePromise<ApiResponse_RuntimeResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/runtimes",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Pack not found`,
        409: `Runtime with same ref already exists`,
      },
    });
  }
  /**
   * @returns ApiResponse_RuntimeResponse Runtime details
   * @throws ApiError
   */
  public static getRuntime({
    ref,
  }: {
    /**
     * Runtime reference identifier
     */
    ref: string;
  }): CancelablePromise<ApiResponse_RuntimeResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/runtimes/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Runtime not found`,
      },
    });
  }
  /**
   * @returns ApiResponse_RuntimeResponse Runtime updated successfully
   * @throws ApiError
   */
  public static updateRuntime({
    ref,
    requestBody,
  }: {
    /**
     * Runtime reference identifier
     */
    ref: string;
    requestBody: UpdateRuntimeRequest;
  }): CancelablePromise<ApiResponse_RuntimeResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/runtimes/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Runtime not found`,
      },
    });
  }
  /**
   * @returns SuccessResponse Runtime deleted successfully
   * @throws ApiError
   */
  public static deleteRuntime({
    ref,
  }: {
    /**
     * Runtime reference identifier
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/runtimes/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Runtime not found`,
      },
    });
  }
}
