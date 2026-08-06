/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheEntryResponse } from "./CacheEntryResponse";
import type { i64 } from "./i64";
/**
 * Point lookup response. `item = None` is an authorized miss.
 */
export type CachePointLookupResponse = {
  generation_id: i64;
  item: null | CacheEntryResponse;
  stale: boolean;
};
