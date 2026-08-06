/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { OwnerType } from "./OwnerType";
/**
 * Seal a staging generation into `ready`.
 */
export type SealCacheGenerationRequest = {
  expected_chunk_count: number;
  expected_record_count?: number | null;
  expected_size_bytes?: number | null;
  owner_ref?: string | null;
  owner_type: OwnerType;
};
