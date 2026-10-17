/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PolicyInputApprovalRequest } from "./PolicyInputApprovalRequest";
import type { RendererInputApprovalRequest } from "./RendererInputApprovalRequest";
/**
 * Canonical flat JSON request. Exactly one target form is accepted; purpose is
 * bound by the server's retained declaration, never selected by this request.
 */
export type IssueDeclaredInputApprovalRequest =
  RendererInputApprovalRequest | PolicyInputApprovalRequest;
