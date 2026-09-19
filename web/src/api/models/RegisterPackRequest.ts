/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AbsentMetadataPolicy } from "./AbsentMetadataPolicy";
/**
 * Request DTO for registering a pack from local filesystem
 */
export type RegisterPackRequest = {
  /**
   * How to handle pack-managed metadata omitted by this release.
   */
  absent_metadata_policy?: AbsentMetadataPolicy;
  /**
   * Force registration even if tests fail
   */
  force?: boolean;
  /**
   * Local filesystem path to the pack directory
   */
  path: string;
  /**
   * Skip running pack tests during registration
   */
  skip_tests?: boolean;
};
