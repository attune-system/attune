/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { IdentityRoleAssignmentResponse } from "./IdentityRoleAssignmentResponse";
import type { PermissionAssignmentResponse } from "./PermissionAssignmentResponse";
import type { Value } from "./Value";
/**
 * Standard API response wrapper
 */
export type ApiResponse_IdentityResponse = {
  data: {
    attributes: Value;
    direct_permissions: Array<PermissionAssignmentResponse>;
    display_name?: string | null;
    frozen: boolean;
    id: number;
    login: string;
    roles: Array<IdentityRoleAssignmentResponse>;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
