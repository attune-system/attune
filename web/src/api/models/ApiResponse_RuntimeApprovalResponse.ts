/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApprovalPurpose } from "./ApprovalPurpose";
import type { RbacGrant } from "./RbacGrant";
/**
 * Standard API response wrapper
 */
export type ApiResponse_RuntimeApprovalResponse = {
  data: {
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
  };
  /**
   * Optional message
   */
  message?: string | null;
};
