/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { BuildSummary } from "./BuildSummary";
import type { BuiltEnvironment } from "./BuiltEnvironment";
import type { FailedEnvironment } from "./FailedEnvironment";
/**
 * Standard API response wrapper
 */
export type ApiResponse_BuildPackEnvsResponse = {
  /**
   * Response DTO for build pack environments operation
   */
  data: {
    /**
     * Successfully built environments
     */
    built_environments: Array<BuiltEnvironment>;
    /**
     * Failed environment builds
     */
    failed_environments: Array<FailedEnvironment>;
    /**
     * Summary statistics
     */
    summary: BuildSummary;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
