/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheEntryUpload } from "./CacheEntryUpload";
import type { OwnerType } from "./OwnerType";
/**
 * Upload one numbered ingest chunk.
 */
export type UploadCacheChunkRequest = {
  /**
   * Delete these IDs from a staging clone. Missing IDs are harmless.
   */
  deleted_external_ids?: Array<string>;
  /**
   * Inserts for a full snapshot, or complete replacement values for a clone.
   */
  entries?: Array<CacheEntryUpload>;
  owner_ref?: string | null;
  owner_type: OwnerType;
};
