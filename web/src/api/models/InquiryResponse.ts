/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { InquiryResponseOption } from "./InquiryResponseOption";
import type { InquiryStatus } from "./InquiryStatus";
/**
 * Full inquiry response with all details
 */
export type InquiryResponse = {
  assigned_to?: null | i64;
  assigned_to_display_name?: string | null;
  assigned_to_login?: string | null;
  /**
   * Creation timestamp
   */
  created: string;
  created_by_action_ref?: string | null;
  /**
   * Execution ID that created this inquiry
   */
  created_by_execution: i64;
  created_by_pack_ref?: string | null;
  /**
   * Inquiry ID
   */
  id: i64;
  /**
   * Prompt text displayed to the user
   */
  prompt: string;
  purpose?: string | null;
  /**
   * When the inquiry was responded to
   */
  responded_at?: string | null;
  responded_by?: null | i64;
  responded_by_display_name?: string | null;
  responded_by_login?: string | null;
  /**
   * Response data provided by the user
   */
  response: any | null;
  /**
   * Fixed responses that provider controls may select.
   */
  response_options: Array<InquiryResponseOption>;
  /**
   * Attune flat schema for expected response fields
   */
  response_schema: any | null;
  /**
   * Current status of the inquiry
   */
  status: InquiryStatus;
  /**
   * When the inquiry expires
   */
  timeout_at?: string | null;
  /**
   * Last update timestamp
   */
  updated: string;
  workflow_action_ref?: string | null;
  workflow_execution?: null | i64;
  workflow_pack_ref?: string | null;
  workflow_root_execution?: null | i64;
  workflow_task_name?: string | null;
};
