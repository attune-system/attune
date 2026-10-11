/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheEntryUpload } from "./CacheEntryUpload";
import type { OwnerType } from "./OwnerType";
/**
 * Atomic mutations to a durable live namespace. No snapshot lifecycle is used.
 */
export type ApplyLiveCacheRequest = {
  deleted_external_ids?: Array<string>;
  entries?: Array<CacheEntryUpload>;
  /**
   * Required optimistic guard. Use zero for the first batch.
   */
  expected_revision: number;
  owner_ref?: string | null;
  owner_type: OwnerType;
  request_id: string;
  /**
   * Omission or null retains the current upstream cursor.
   */
  source_revision?: string | null;
};
