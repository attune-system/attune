/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AbsentMetadataPolicy } from "./AbsentMetadataPolicy";
/**
 * Request DTO for installing a pack from remote source
 */
export type InstallPackRequest = {
  /**
   * How to handle pack-managed metadata omitted by this release.
   */
  absent_metadata_policy?: AbsentMetadataPolicy;
  /**
   * Replace an existing pack with the same ref
   */
  force?: boolean;
  /**
   * Require an explicit URL or existing local path instead of registry lookup.
   */
  no_registry?: boolean;
  /**
   * Git branch, tag, or commit reference
   */
  ref_spec?: string | null;
  /**
   * Restrict registry-reference resolution to one managed index.
   */
  registry_id?: number | null;
  /**
   * Skip dependency validation (not recommended)
   */
  skip_deps?: boolean;
  /**
   * Skip running pack tests during installation
   */
  skip_tests?: boolean;
  /**
   * Repository URL or source location
   */
  source: string;
};
