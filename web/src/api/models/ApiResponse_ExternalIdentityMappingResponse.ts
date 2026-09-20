/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_ExternalIdentityMappingResponse = {
  data: {
    created: string;
    created_by?: number | null;
    external_subject: string;
    id: number;
    integration_identity: number;
    mapped_identity: number;
    provider: string;
    tenant: string;
    updated: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
