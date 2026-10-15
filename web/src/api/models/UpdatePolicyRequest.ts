/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ConcurrencyPolicyRequest } from "./ConcurrencyPolicyRequest";
import type { QuotaPolicyRequest } from "./QuotaPolicyRequest";
import type { RateLimitPolicyRequest } from "./RateLimitPolicyRequest";
export type UpdatePolicyRequest = {
  concurrency?: null | ConcurrencyPolicyRequest;
  description?: string | null;
  enabled?: boolean | null;
  name?: string | null;
  priority?: number | null;
  quotas?: Array<QuotaPolicyRequest> | null;
  rate_limit?: null | RateLimitPolicyRequest;
  tags?: Array<string> | null;
};
