/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
/**
 * Summary key response for list views (value redacted)
 */
export type KeySummary = {
  /**
   * Creation timestamp
   */
  created: string;
  /**
   * Whether the value is encrypted
   */
  encrypted: boolean;
  /**
   * Unique key ID
   */
  id: i64;
  /**
   * Key identifier within its owner scope
   */
  local_ref: string;
  /**
   * Human-readable name
   */
  name: string;
  /**
   * Authoritative owner reference
   */
  owner?: string | null;
  /**
   * Type of owner
   */
  owner_type: OwnerType;
  /**
   * Unique reference identifier
   */
  ref: string;
};
