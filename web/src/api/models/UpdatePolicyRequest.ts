/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ConcurrencyPolicyRequest } from "./ConcurrencyPolicyRequest";
import type { RateLimitPolicyRequest } from "./RateLimitPolicyRequest";
export type UpdatePolicyRequest = {
  concurrency?: null | ConcurrencyPolicyRequest;
  description?: string | null;
  enabled?: boolean | null;
  name?: string | null;
  priority?: number | null;
  quotas?: any[] | null;
  rate_limit?: null | RateLimitPolicyRequest;
  tags?: any[] | null;
};
