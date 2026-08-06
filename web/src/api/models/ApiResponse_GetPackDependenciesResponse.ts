/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AnalyzedPack } from "./AnalyzedPack";
import type { DependencyError } from "./DependencyError";
import type { PackDependency } from "./PackDependency";
import type { RuntimeRequirements } from "./RuntimeRequirements";
/**
 * Standard API response wrapper
 */
export type ApiResponse_GetPackDependenciesResponse = {
  /**
   * Response DTO for get pack dependencies operation
   */
  data: {
    /**
     * Packs that were analyzed
     */
    analyzed_packs: Array<AnalyzedPack>;
    /**
     * All dependencies found
     */
    dependencies: Array<PackDependency>;
    /**
     * Errors encountered during analysis
     */
    errors: Array<DependencyError>;
    /**
     * Dependencies not yet installed
     */
    missing_dependencies: Array<PackDependency>;
    /**
     * Runtime requirements by pack
     */
    runtime_requirements: Record<string, RuntimeRequirements>;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
