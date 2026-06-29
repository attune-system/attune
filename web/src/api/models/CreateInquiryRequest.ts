/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
/**
 * Request to create a new inquiry
 */
export type CreateInquiryRequest = {
  assigned_to?: null | i64;
  /**
   * Execution ID this inquiry belongs to
   */
  execution: i64;
  /**
   * Prompt text to display to the user
   */
  prompt: string;
  /**
   * Optional schema for the expected response format (flat format with inline required/secret)
   */
  response_schema: Record<string, any>;
  /**
   * Optional timeout timestamp (when inquiry expires)
   */
  timeout_at?: string | null;
};
