/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
/**
 * Tombstone/queued-cleanup status returned by namespace deletion.
 */
export type CacheNamespaceDeletionResponse = {
  /**
   * Cleanup is asynchronous; entries are reclaimed in bounded batches.
   */
  cleanup_pending: boolean;
  id: i64;
  namespace: string;
  status: string;
  tombstoned: boolean;
};
