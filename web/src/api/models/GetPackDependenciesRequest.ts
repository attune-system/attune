/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for getting pack dependencies
 */
export type GetPackDependenciesRequest = {
  /**
   * List of pack directory paths to analyze
   */
  pack_paths: Array<string>;
  /**
   * Skip pack.yaml validation
   */
  skip_validation?: boolean;
};
