/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { InquiryStatus } from "./InquiryStatus";
/**
 * Standard API response wrapper
 */
export type ApiResponse_InquiryResponse = {
  /**
   * Full inquiry response with all details
   */
  data: {
    assigned_to?: null | i64;
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Execution ID this inquiry belongs to
     */
    execution: i64;
    /**
     * Inquiry ID
     */
    id: i64;
    /**
     * Prompt text displayed to the user
     */
    prompt: string;
    /**
     * When the inquiry was responded to
     */
    responded_at?: string | null;
    /**
     * Response data provided by the user
     */
    response: any | null;
    /**
     * JSON schema for expected response
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
  };
  /**
   * Optional message
   */
  message?: string | null;
};
