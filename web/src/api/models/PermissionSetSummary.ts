/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ManagementOriginKind } from "./ManagementOriginKind";
import type { PermissionSetRoleAssignmentResponse } from "./PermissionSetRoleAssignmentResponse";
import type { Value } from "./Value";
export type PermissionSetSummary = {
  description?: string | null;
  grants: Value;
  id: number;
  label?: string | null;
  management_origin: ManagementOriginKind;
  pack_ref?: string | null;
  ref: string;
  retired_at?: string | null;
  roles: Array<PermissionSetRoleAssignmentResponse>;
};
