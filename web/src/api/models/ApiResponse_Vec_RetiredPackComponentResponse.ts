/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_Vec_RetiredPackComponentResponse = {
  data: Array<{
    component_ref?: string | null;
    id: number;
    kind: string;
    managed_release?: number | null;
    retired_at: string;
  }>;
  /**
   * Optional message
   */
  message?: string | null;
};
