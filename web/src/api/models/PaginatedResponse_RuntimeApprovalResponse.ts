/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApprovalPurpose } from "./ApprovalPurpose";
import type { PaginationMeta } from "./PaginationMeta";
import type { RbacGrant } from "./RbacGrant";
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
    grants: Array<RbacGrant>;
    id: number;
    issuer_identity_id: number;
    issuer_login: string;
    purpose: ApprovalPurpose;
    revoked_at?: string | null;
    subject_pack_id: number;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
