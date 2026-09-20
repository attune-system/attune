/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { InquiryResponse } from "./InquiryResponse";
/**
 * Standard API response wrapper
 */
export type ApiResponse_CreateInquiryResponse = {
  /**
   * Creation result containing the inquiry and its provider-neutral response handle.
   */
  data: {
    inquiry: InquiryResponse;
    /**
     * Opaque correlation handle for one-shot external responses.
     */
    response_handle: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
