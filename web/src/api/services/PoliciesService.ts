/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_PolicyResponse } from "../models/ApiResponse_PolicyResponse";
import type { ApiResponse_SuccessResponse } from "../models/ApiResponse_SuccessResponse";
import type { CreatePolicyRequest } from "../models/CreatePolicyRequest";
import type { PaginatedResponse_PolicySummary } from "../models/PaginatedResponse_PolicySummary";
import type { PolicyScopeType } from "../models/PolicyScopeType";
import type { UpdatePolicyRequest } from "../models/UpdatePolicyRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class PoliciesService {
  /**
   * @returns PaginatedResponse_PolicySummary List of policies for an action
   * @throws ApiError
   */
  public static listPoliciesByAction({
    actionRef,
    page,
    pageSize,
  }: {
    /**
     * Action reference
     */
    actionRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_PolicySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/actions/{action_ref}/policies",
      path: {
        action_ref: actionRef,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
    });
  }
  /**
   * @returns PaginatedResponse_PolicySummary List of policies for a pack
   * @throws ApiError
   */
  public static listPoliciesByPack({
    packRef,
    page,
    pageSize,
  }: {
    /**
     * Pack reference
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
  }): CancelablePromise<PaginatedResponse_PolicySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/policies",
      path: {
        pack_ref: packRef,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
    });
  }
  /**
   * @returns PaginatedResponse_PolicySummary List of policies
   * @throws ApiError
   */
  public static listPolicies({
    page,
    pageSize,
    packRef,
    actionRef,
    scope,
    enabled,
    tag,
  }: {
    page?: number;
    pageSize?: number;
    packRef?: string | null;
    actionRef?: string | null;
    scope?: null | PolicyScopeType;
    enabled?: boolean | null;
    tag?: string | null;
  }): CancelablePromise<PaginatedResponse_PolicySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/policies",
      query: {
        page: page,
        page_size: pageSize,
        pack_ref: packRef,
        action_ref: actionRef,
        scope: scope,
        enabled: enabled,
        tag: tag,
      },
    });
  }
  /**
   * @returns ApiResponse_PolicyResponse Policy created
   * @throws ApiError
   */
  public static createPolicy({
    requestBody,
  }: {
    requestBody: CreatePolicyRequest;
  }): CancelablePromise<ApiResponse_PolicyResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/policies",
      body: requestBody,
      mediaType: "application/json",
    });
  }
  /**
   * @returns ApiResponse_PolicyResponse Policy details
   * @throws ApiError
   */
  public static getPolicy({
    ref,
  }: {
    /**
     * Policy reference
     */
    ref: string;
  }): CancelablePromise<ApiResponse_PolicyResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/policies/{ref}",
      path: {
        ref: ref,
      },
    });
  }
  /**
   * @returns ApiResponse_PolicyResponse Policy updated
   * @throws ApiError
   */
  public static updatePolicy({
    ref,
    requestBody,
  }: {
    /**
     * Policy reference
     */
    ref: string;
    requestBody: UpdatePolicyRequest;
  }): CancelablePromise<ApiResponse_PolicyResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/policies/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
    });
  }
  /**
   * @returns ApiResponse_SuccessResponse Policy deleted
   * @throws ApiError
   */
  public static deletePolicy({
    ref,
  }: {
    /**
     * Policy reference
     */
    ref: string;
  }): CancelablePromise<ApiResponse_SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/policies/{ref}",
      path: {
        ref: ref,
      },
    });
  }
}
