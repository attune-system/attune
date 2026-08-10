/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Information about an analyzed pack
 */
export type AnalyzedPack = {
  /**
   * Number of dependencies
   */
  dependency_count: number;
  /**
   * Whether pack has dependencies
   */
  has_dependencies: boolean;
  /**
   * Pack directory path
   */
  pack_path: string;
  /**
   * Pack reference
   */
  pack_ref: string;
};
