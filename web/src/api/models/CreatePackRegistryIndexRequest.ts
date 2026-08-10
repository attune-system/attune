/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request to add a configured pack registry index.
 */
export type CreatePackRegistryIndexRequest = {
  enabled?: boolean;
  headers?: Record<string, any>;
  name?: string | null;
  /**
   * Optional explicit search order position. Omit to append to the end.
   */
  position?: number | null;
  url: string;
};
