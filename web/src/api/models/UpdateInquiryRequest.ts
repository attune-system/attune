/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { InquiryStatus } from "./InquiryStatus";
/**
 * Request to update an inquiry
 */
export type UpdateInquiryRequest = {
  assigned_to?: null | i64;
  /**
   * Update the response data
   */
  response: any | null;
  status?: null | InquiryStatus;
};
