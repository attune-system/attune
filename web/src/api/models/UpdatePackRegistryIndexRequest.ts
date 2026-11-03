/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request to update a configured pack registry index.
 */
export type UpdatePackRegistryIndexRequest = {
  enabled?: boolean | null;
  headers?: Record<string, any> | null;
  name?: string | null;
  position?: number | null;
  /**
   * Omit to preserve existing source headers; use {} to clear them.
   */
  source_headers?: Record<string, any> | null;
  url?: string | null;
};
