/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
/**
 * Atomically promote a ready generation.
 */
export type PromoteCacheGenerationRequest = {
  expected_active_generation_id: null | i64;
  owner_ref?: string | null;
  owner_type: OwnerType;
};
