/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Response DTO for pack information
 */
export type PackResponse = {
  /**
   * Configuration schema
   */
  conf_schema: Record<string, any>;
  /**
   * Pack configuration
   */
  config: Record<string, any>;
  /**
   * Creation timestamp
   */
  created: string;
  /**
   * Pack dependencies (refs of required packs)
   */
  dependencies: Array<string>;
  /**
   * Pack description
   */
  description?: string | null;
  /**
   * Pack ID
   */
  id: number;
  /**
   * Is standard pack
   */
  is_standard: boolean;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Pack metadata
   */
  meta: Record<string, any>;
  /**
   * Unique reference identifier
   */
  ref: string;
  /**
   * Runtime dependencies (e.g., shell, python, nodejs)
   */
  runtime_deps: Array<string>;
  /**
   * Tags
   */
  tags: Array<string>;
  /**
   * Last update timestamp
   */
  updated: string;
  /**
   * Pack version
   */
  version: string;
};
