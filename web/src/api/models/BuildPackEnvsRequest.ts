/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for building pack environments
 */
export type BuildPackEnvsRequest = {
  /**
   * Force rebuild of existing environments
   */
  force_rebuild?: boolean;
  /**
   * Node.js version to use
   */
  nodejs_version?: string;
  /**
   * List of pack directory paths
   */
  pack_paths: Array<string>;
  /**
   * Base directory for permanent pack storage
   */
  packs_base_dir?: string | null;
  /**
   * Python version to use
   */
  python_version?: string;
  /**
   * Skip building Node.js environments
   */
  skip_nodejs?: boolean;
  /**
   * Skip building Python environments
   */
  skip_python?: boolean;
  /**
   * Timeout in seconds for building each environment
   */
  timeout?: number;
};
