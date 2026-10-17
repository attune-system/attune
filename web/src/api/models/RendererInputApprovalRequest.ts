/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PermissionGrantDefinition } from "./PermissionGrantDefinition";
export type RendererInputApprovalRequest = {
  declaration_revision: string;
  expires_at?: string | null;
  expression: string;
  grants: Array<PermissionGrantDefinition>;
  /**
   * Convenience target for an already admitted renderer, not a policy target.
   */
  renderer_execution: number;
  source_path: string;
};
