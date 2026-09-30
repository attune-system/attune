/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_BuildInfo = {
  /**
   * Identity compiled into this binary, never read from deployment-time environment variables.
   */
  data: {
    /**
     * Full source commit SHA, or "unknown" when the build had no revision metadata.
     */
    git_sha: string;
    /**
     * Semantic version of the platform workspace.
     */
    version: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
