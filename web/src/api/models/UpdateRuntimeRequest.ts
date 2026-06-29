/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { NullableJsonPatch } from "./NullableJsonPatch";
import type { NullableStringPatch } from "./NullableStringPatch";
/**
 * Request DTO for updating a runtime.
 */
export type UpdateRuntimeRequest = {
  description?: null | NullableStringPatch;
  /**
   * Distribution metadata used for verification and platform support
   */
  distributions: any | null;
  /**
   * Runtime execution configuration
   */
  execution_config: any | null;
  installation?: null | NullableJsonPatch;
  /**
   * Display name
   */
  name?: string | null;
};
