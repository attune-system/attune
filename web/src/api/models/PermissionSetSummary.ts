/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PermissionSetRoleAssignmentResponse } from "./PermissionSetRoleAssignmentResponse";
import type { Value } from "./Value";
export type PermissionSetSummary = {
  description?: string | null;
  grants: Value;
  id: number;
  label?: string | null;
  pack_ref?: string | null;
  ref: string;
  roles: Array<PermissionSetRoleAssignmentResponse>;
};
