/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Identity compiled into this binary, never read from deployment-time environment variables.
 */
export type BuildInfo = {
  /**
   * Full source commit SHA, or "unknown" when the build had no revision metadata.
   */
  git_sha: string;
  /**
   * Semantic version of the platform workspace.
   */
  version: string;
};
