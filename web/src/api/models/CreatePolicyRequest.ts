/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ConcurrencyPolicyRequest } from "./ConcurrencyPolicyRequest";
import type { PolicyScopeRequest } from "./PolicyScopeRequest";
import type { QuotaPolicyRequest } from "./QuotaPolicyRequest";
import type { RateLimitPolicyRequest } from "./RateLimitPolicyRequest";
import type { ResourceConcurrency } from "./ResourceConcurrency";
export type CreatePolicyRequest = {
  concurrency?: null | ConcurrencyPolicyRequest;
  description?: string | null;
  enabled?: boolean;
  name: string;
  priority?: number;
  quotas?: Array<QuotaPolicyRequest>;
  rate_limit?: null | RateLimitPolicyRequest;
  ref: string;
  resource_concurrency?: null | ResourceConcurrency;
  scope: PolicyScopeRequest;
  tags?: Array<string>;
};
