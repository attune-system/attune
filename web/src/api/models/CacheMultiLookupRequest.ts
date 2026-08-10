/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
/**
 * Bounded multi-ID lookup request.
 */
export type CacheMultiLookupRequest = {
  external_ids: Array<string>;
  generation_id?: null | i64;
  owner_ref?: string | null;
  owner_type: OwnerType;
  require_fresh?: boolean;
};
