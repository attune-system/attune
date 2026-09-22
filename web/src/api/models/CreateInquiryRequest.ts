/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { InquiryResponseOption } from "./InquiryResponseOption";
/**
 * Request to create a new inquiry
 */
export type CreateInquiryRequest = {
  assigned_to?: null | i64;
  /**
   * Prompt text to display to the user
   */
  prompt: string;
  /**
   * Stable purpose used to make creation idempotent within this workflow task attempt.
   */
  purpose: string;
  /**
   * Fixed response choices rendered by provider actions.
   */
  response_options: Array<InquiryResponseOption>;
  /**
   * Optional schema for the expected response format (flat format with inline required/secret)
   */
  response_schema?: any | null;
  /**
   * Optional relative timeout in seconds.
   */
  timeout_seconds?: number | null;
};
