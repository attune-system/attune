/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_ExternalIdentityMappingResponse = {
  /**
   * The page items
   */
  items: Array<{
    created: string;
    created_by?: number | null;
    external_subject: string;
    id: number;
    integration_identity: number;
    mapped_identity: number;
    provider: string;
    tenant: string;
    updated: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
