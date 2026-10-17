/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { InvocationBinding } from "./InvocationBinding";
import type { PermissionGrantDefinition } from "./PermissionGrantDefinition";
export type PolicyInputApprovalRequest = {
  declaration_revision: string;
  expires_at?: string | null;
  expression: string;
  grants: Array<PermissionGrantDefinition>;
  invocation_binding: InvocationBinding;
  /**
   * Pre-execution target, supplied with invocation_binding, without a renderer.
   */
  policy_revision: number;
  source_path: string;
};
