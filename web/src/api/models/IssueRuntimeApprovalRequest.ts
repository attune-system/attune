/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type IssueRuntimeApprovalRequest = {
  /**
   * Omit for no automatic expiry. Explicit revocation remains available.
   */
  expires_at?: string | null;
  /**
   * Fixed approved grants. Human-relative scopes are not accepted.
   */
  grants: Array<Record<string, any>>;
};
