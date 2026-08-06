/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheGenerationState } from "./CacheGenerationState";
import type { i64 } from "./i64";
/**
 * Immutable generation metadata. Also serves as the refresh-lifecycle
 * operation response for create/upload/seal/promote/abandon.
 */
export type CacheGenerationResponse = {
  activated: string | null;
  checksum: string | null;
  checksum_algorithm: string | null;
  client_refresh_id: string;
  created: string;
  created_by: null | i64;
  expected_active_generation_id: null | i64;
  expected_chunk_count: number;
  expected_record_count: number | null;
  expected_size_bytes: number | null;
  failed: string | null;
  failure_reason: string | null;
  /**
   * Generation identifier (accepted by the client as `generation`/`id`).
   */
  generation_id: i64;
  namespace_id: i64;
  readable_until: string | null;
  record_count: number;
  retired: string | null;
  sealed: string | null;
  size_bytes: number;
  source_revision: string | null;
  /**
   * Lifecycle state: `staging`, `ready`, `active`, `retired`, or `failed`.
   */
  status: CacheGenerationState;
};
