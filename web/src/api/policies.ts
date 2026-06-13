import type { CancelablePromise } from "./core/CancelablePromise";
import { OpenAPI } from "./core/OpenAPI";
import { request as __request } from "./core/request";

export interface ApiResponse<T> {
  data: T;
  message?: string | null;
}

export interface PaginatedResponse<T> {
  items: T[];
  meta: {
    page: number;
    page_size: number;
    total: number;
    total_pages: number;
  };
}

export type PolicyScope = "global" | "pack" | "action";
export type PolicyMethod = "cancel" | "enqueue";

export interface PolicyResponse {
  id: number;
  ref: string;
  scope: PolicyScope;
  pack?: number | null;
  pack_ref?: string | null;
  action?: number | null;
  action_ref?: string | null;
  parameters: string[];
  method: PolicyMethod;
  threshold: number;
  name: string;
  description?: string | null;
  tags: string[];
  created: string;
  updated: string;
}

export type PolicySummary = PolicyResponse;

export interface CreatePolicyRequest {
  ref: string;
  pack_ref?: string | null;
  action_ref?: string | null;
  parameters: string[];
  method: PolicyMethod;
  threshold: number;
  name: string;
  description?: string | null;
  tags: string[];
}

export interface UpdatePolicyRequest {
  parameters?: string[];
  method?: PolicyMethod;
  threshold?: number;
  name?: string;
  description?: string | null;
  tags?: string[];
}

export class PoliciesService {
  public static listPolicies({
    page = 1,
    pageSize = 50,
    scope,
    packRef,
    actionRef,
  }: {
    page?: number;
    pageSize?: number;
    scope?: PolicyScope;
    packRef?: string;
    actionRef?: string;
  } = {}): CancelablePromise<PaginatedResponse<PolicySummary>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/policies",
      query: {
        page,
        page_size: pageSize,
        scope,
        pack_ref: packRef,
        action_ref: actionRef,
      },
      errors: {
        403: "Insufficient permissions",
      },
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
        400: "Invalid policy",
        403: "Insufficient permissions",
        404: "Referenced action or pack not found",
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
        400: "Invalid policy",
        403: "Insufficient permissions",
        404: "Policy not found",
      },
    });
  }

  public static deletePolicy({ ref }: { ref: string }): CancelablePromise<unknown> {
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
