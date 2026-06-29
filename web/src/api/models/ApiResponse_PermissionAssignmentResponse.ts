/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_PermissionAssignmentResponse = {
  data: {
    created: string;
    id: number;
    identity_id: number;
    permission_set_id: number;
    permission_set_ref: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
