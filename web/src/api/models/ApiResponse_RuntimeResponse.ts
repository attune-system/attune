/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_RuntimeResponse = {
  /**
   * Full runtime response.
   */
  data: {
    created: string;
    description?: string | null;
    distributions: Record<string, any>;
    execution_config: Record<string, any>;
    id: number;
    installation: any | null;
    name: string;
    pack?: number | null;
    pack_ref?: string | null;
    ref: string;
    updated: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
