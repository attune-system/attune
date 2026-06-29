/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type IntegrationTokenResponse = {
  active: boolean;
  created: string;
  created_by?: number | null;
  description?: string | null;
  expires_at?: string | null;
  id: number;
  identity_id: number;
  label: string;
  last_used_at?: string | null;
  last_used_ip?: string | null;
  revocation_reason?: string | null;
  revoked_at?: string | null;
  revoked_by?: number | null;
  token_prefix: string;
  token_suffix: string;
  updated: string;
};
