/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Build summary statistics
 */
export type BuildSummary = {
  /**
   * Failed builds
   */
  failure_count: number;
  /**
   * Node.js environments built
   */
  nodejs_envs_built: number;
  /**
   * Python environments built
   */
  python_envs_built: number;
  /**
   * Successfully built
   */
  success_count: number;
  /**
   * Total duration in milliseconds
   */
  total_duration_ms: number;
  /**
   * Total packs processed
   */
  total_packs: number;
};
