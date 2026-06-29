/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { IdentityRoleAssignmentResponse } from "./IdentityRoleAssignmentResponse";
import type { PermissionAssignmentResponse } from "./PermissionAssignmentResponse";
import type { Value } from "./Value";
export type IdentityResponse = {
  attributes: Value;
  direct_permissions: Array<PermissionAssignmentResponse>;
  display_name?: string | null;
  frozen: boolean;
  id: number;
  login: string;
  roles: Array<IdentityRoleAssignmentResponse>;
};
