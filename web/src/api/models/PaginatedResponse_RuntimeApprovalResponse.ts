/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_RuntimeApprovalResponse = {
  /**
   * The page items
   */
  items: Array<{
    active: boolean;
    approval_digest: string;
    expires_at?: string | null;
    grants: Array<Record<string, any>>;
    id: number;
    issuer_identity_id: number;
    issuer_login: string;
    revoked_at?: string | null;
    subject_pack_id: number;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
