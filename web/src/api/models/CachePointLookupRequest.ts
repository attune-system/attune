/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
/**
 * Point lookup request. Identifiers are placed in the body to avoid access-log
 * leakage.
 */
export type CachePointLookupRequest = {
  external_id: string;
  generation_id?: null | i64;
  owner_ref?: string | null;
  owner_type: OwnerType;
  require_fresh?: boolean;
};
