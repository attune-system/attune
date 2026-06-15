import type { CancelablePromise } from "./core/CancelablePromise";
import { OpenAPI } from "./core/OpenAPI";
import { request as __request } from "./core/request";

export interface PaginationMeta {
  page: number;
  page_size: number;
  total_items?: number | null;
  total_pages?: number | null;
  has_previous: boolean;
  has_next: boolean;
}

export interface PaginatedApiResponse<T> {
  items: T[];
  pagination: PaginationMeta;
}

export interface ApiResponse<T> {
  data: T;
  message?: string | null;
}

export interface SuccessResponse {
  success: boolean;
  message: string;
}

export enum PolicyScopeType {
  GLOBAL = "global",
  PACK = "pack",
  ACTION = "action",
}

export enum PolicyMethod {
  CANCEL = "cancel",
  ENQUEUE = "enqueue",
}

export interface PolicyScopeRequest {
  type: PolicyScopeType;
  pack_ref?: string | null;
  action_ref?: string | null;
}

export interface PolicyScopeResponse extends PolicyScopeRequest {
  pack?: number | null;
  action?: number | null;
}

export interface ConcurrencyPolicyRequest {
  limit: number;
  method: PolicyMethod;
  parameters?: string[];
}

export interface RateLimitPolicyRequest {
  max_executions: number;
  window_seconds: number;
}

export interface QuotaPolicyRequest {
  quota_type: string;
  limit: number;
}

export interface PolicySummary {
  id: number;
  ref: string;
  name: string;
  description?: string | null;
  enabled: boolean;
  priority: number;
  scope: PolicyScopeResponse;
  concurrency?: ConcurrencyPolicyRequest | null;
  rate_limit?: RateLimitPolicyRequest | null;
  quotas: QuotaPolicyRequest[];
  tags: string[];
  created: string;
  updated: string;
}

export type PolicyResponse = PolicySummary;

export interface CreatePolicyRequest {
  ref: string;
  name: string;
  description?: string | null;
  enabled?: boolean;
  priority?: number;
  scope: PolicyScopeRequest;
  concurrency?: ConcurrencyPolicyRequest | null;
  rate_limit?: RateLimitPolicyRequest | null;
  quotas?: QuotaPolicyRequest[];
  tags?: string[];
}

export interface UpdatePolicyRequest {
  name?: string;
  description?: string | null;
  enabled?: boolean;
  priority?: number;
  concurrency?: ConcurrencyPolicyRequest | null;
  rate_limit?: RateLimitPolicyRequest | null;
  quotas?: QuotaPolicyRequest[];
  tags?: string[];
}

export interface ListPoliciesParams {
  page?: number;
  pageSize?: number;
  packRef?: string;
  actionRef?: string;
  scope?: PolicyScopeType;
  enabled?: boolean;
  tag?: string;
}

export class PoliciesService {
  public static listPolicies({
    page,
    pageSize,
    packRef,
    actionRef,
    scope,
    enabled,
    tag,
  }: ListPoliciesParams = {}): CancelablePromise<PaginatedApiResponse<PolicySummary>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/policies",
      query: {
        page,
        page_size: pageSize,
        pack_ref: packRef,
        action_ref: actionRef,
        scope,
        enabled,
        tag,
      },
    });
  }

  public static listPoliciesByPack({
    packRef,
    page,
    pageSize,
  }: {
    packRef: string;
    page?: number;
    pageSize?: number;
  }): CancelablePromise<PaginatedApiResponse<PolicySummary>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/policies",
      path: { pack_ref: packRef },
      query: { page, page_size: pageSize },
    });
  }

  public static listPoliciesByAction({
    actionRef,
    page,
    pageSize,
  }: {
    actionRef: string;
    page?: number;
    pageSize?: number;
  }): CancelablePromise<PaginatedApiResponse<PolicySummary>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/actions/{action_ref}/policies",
      path: { action_ref: actionRef },
      query: { page, page_size: pageSize },
    });
  }

  public static getPolicy({
    ref,
  }: {
    ref: string;
  }): CancelablePromise<ApiResponse<PolicyResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/policies/{ref}",
      path: { ref },
      errors: { 404: "Policy not found" },
    });
  }

  public static createPolicy({
    requestBody,
  }: {
    requestBody: CreatePolicyRequest;
  }): CancelablePromise<ApiResponse<PolicyResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/policies",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Validation error",
        403: "Insufficient permissions",
        404: "Referenced pack or action not found",
        409: "Policy already exists",
      },
    });
  }

  public static updatePolicy({
    ref,
    requestBody,
  }: {
    ref: string;
    requestBody: UpdatePolicyRequest;
  }): CancelablePromise<ApiResponse<PolicyResponse>> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/policies/{ref}",
      path: { ref },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Validation error",
        403: "Insufficient permissions",
        404: "Policy not found",
      },
    });
  }

  public static deletePolicy({ ref }: { ref: string }): CancelablePromise<ApiResponse<SuccessResponse>> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/policies/{ref}",
      path: { ref },
      errors: {
        403: "Insufficient permissions",
        404: "Policy not found",
      },
    });
  }
}
