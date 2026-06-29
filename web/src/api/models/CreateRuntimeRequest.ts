/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for creating a runtime.
 */
export type CreateRuntimeRequest = {
  /**
   * Optional human-readable description
   */
  description?: string | null;
  /**
   * Distribution metadata used for verification and platform support
   */
  distributions?: Record<string, any>;
  /**
   * Runtime execution configuration
   */
  execution_config?: Record<string, any>;
  /**
   * Optional installation metadata
   */
  installation?: any | null;
  /**
   * Display name
   */
  name: string;
  /**
   * Optional pack reference this runtime belongs to
   */
  pack_ref?: string | null;
  /**
   * Unique reference identifier (e.g. "core.python", "core.nodejs")
   */
  ref: string;
};
