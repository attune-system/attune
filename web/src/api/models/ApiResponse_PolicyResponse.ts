/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ConcurrencyPolicyResponse } from "./ConcurrencyPolicyResponse";
import type { PolicyScopeResponse } from "./PolicyScopeResponse";
import type { QuotaPolicyResponse } from "./QuotaPolicyResponse";
import type { RateLimitPolicyResponse } from "./RateLimitPolicyResponse";
/**
 * Standard API response wrapper
 */
export type ApiResponse_PolicyResponse = {
  data: {
    concurrency?: null | ConcurrencyPolicyResponse;
    created: string;
    description?: string | null;
    enabled: boolean;
    id: number;
    name: string;
    priority: number;
    quotas: Array<QuotaPolicyResponse>;
    rate_limit?: null | RateLimitPolicyResponse;
    ref: string;
    scope: PolicyScopeResponse;
    tags: Array<string>;
    updated: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
