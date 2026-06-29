/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for installing a pack from remote source
 */
export type InstallPackRequest = {
  /**
   * Git branch, tag, or commit reference
   */
  ref_spec?: string | null;
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
