/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { InquiryResponse } from "./InquiryResponse";
import type { InquiryResponseOptionHandle } from "./InquiryResponseOptionHandle";
/**
 * Standard API response wrapper
 */
export type ApiResponse_CreateInquiryResponse = {
  /**
   * Creation result containing the inquiry and one opaque handle per response option.
   */
  data: {
    inquiry: InquiryResponse;
    response_options: Array<InquiryResponseOptionHandle>;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
