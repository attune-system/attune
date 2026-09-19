/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AbsentMetadataPolicy } from "./AbsentMetadataPolicy";
/**
 * Request DTO for registering multiple packs
 */
export type RegisterPacksRequest = {
  /**
   * How to handle pack-managed metadata omitted by each release.
   */
  absent_metadata_policy?: AbsentMetadataPolicy;
  /**
   * Force registration (replace if exists)
   */
  force?: boolean;
  /**
   * List of pack directory paths to register
   */
  pack_paths: Array<string>;
  /**
   * Base directory for permanent storage
   */
  packs_base_dir?: string | null;
  /**
   * Skip running pack tests
   */
  skip_tests?: boolean;
  /**
   * Skip schema validation
   */
  skip_validation?: boolean;
};
