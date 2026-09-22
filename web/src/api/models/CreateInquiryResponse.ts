/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { InquiryResponse } from "./InquiryResponse";
import type { InquiryResponseOptionHandle } from "./InquiryResponseOptionHandle";
/**
 * Creation result containing the inquiry and one opaque handle per response option.
 */
export type CreateInquiryResponse = {
  inquiry: InquiryResponse;
  response_options: Array<InquiryResponseOptionHandle>;
};
