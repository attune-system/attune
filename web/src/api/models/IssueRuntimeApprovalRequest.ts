/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PermissionGrantDefinition } from "./PermissionGrantDefinition";
export type IssueRuntimeApprovalRequest = {
  /**
   * Omit for no automatic expiry. Explicit revocation remains available.
   */
  expires_at?: string | null;
  /**
   * Fixed approved grants. Human-relative scopes are not accepted.
   */
  grants: Array<PermissionGrantDefinition>;
};
