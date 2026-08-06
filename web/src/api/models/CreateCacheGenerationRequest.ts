/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
/**
 * Create (begin) a staging generation.
 */
export type CreateCacheGenerationRequest = {
  client_refresh_id: string;
  expected_active_generation_id: null | i64;
  /**
   * Declared chunk count for contiguity validation at seal time.
   */
  expected_chunk_count: number;
  expected_record_count?: number | null;
  expected_size_bytes?: number | null;
  owner_ref?: string | null;
  owner_type: OwnerType;
  source_revision?: string | null;
};
