/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_Vec_PackReleaseResponse = {
  data: Array<{
    archive_size: number;
    created: string;
    digest: string;
    id: number;
    inactive_since?: string | null;
    is_active: boolean;
    version: string;
  }>;
  /**
   * Optional message
   */
  message?: string | null;
};
