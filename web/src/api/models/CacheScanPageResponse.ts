/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheEntryResponse } from "./CacheEntryResponse";
import type { i64 } from "./i64";
/**
 * One generation-pinned scan page.
 */
export type CacheScanPageResponse = {
  cursor_expires_at: string | null;
  generation_id: i64;
  items: Array<CacheEntryResponse>;
  next_cursor: string | null;
  record_count: number | null;
  stale: boolean;
};
