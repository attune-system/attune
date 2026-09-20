/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { InquiryResponse } from "./InquiryResponse";
/**
 * Creation result containing the inquiry and its provider-neutral response handle.
 */
export type CreateInquiryResponse = {
  inquiry: InquiryResponse;
  /**
   * Opaque correlation handle for one-shot external responses.
   */
  response_handle: string;
};
