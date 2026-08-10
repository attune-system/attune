/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheEntryResponse } from "./CacheEntryResponse";
import type { i64 } from "./i64";
/**
 * Bounded multi-ID lookup response. Missing IDs are reported explicitly.
 */
export type CacheMultiLookupResponse = {
  generation_id: i64;
  items: Array<CacheEntryResponse>;
  missing_external_ids: Array<string>;
  stale: boolean;
};
