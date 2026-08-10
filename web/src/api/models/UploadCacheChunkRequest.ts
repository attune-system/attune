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
  entries: Array<CacheEntryUpload>;
  owner_ref?: string | null;
  owner_type: OwnerType;
};
