/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { InquiryStatus } from "./InquiryStatus";
/**
 * Summary inquiry response for list views
 */
export type InquirySummary = {
  assigned_to?: null | i64;
  /**
   * Creation timestamp
   */
  created: string;
  /**
   * Execution ID
   */
  execution: i64;
  /**
   * Whether a response has been provided
   */
  has_response: boolean;
  /**
   * Inquiry ID
   */
  id: i64;
  /**
   * Prompt text
   */
  prompt: string;
  /**
   * Inquiry status
   */
  status: InquiryStatus;
  /**
   * Timeout timestamp
   */
  timeout_at?: string | null;
};
