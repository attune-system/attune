/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for creating a new pack
 */
export type CreatePackRequest = {
  /**
   * Configuration schema (flat format with inline required/secret per parameter)
   */
  conf_schema?: Record<string, any>;
  /**
   * Pack configuration values
   */
  config?: Record<string, any>;
  /**
   * Pack dependencies (refs of required packs)
   */
  dependencies?: Array<string>;
  /**
   * Pack description
   */
  description?: string | null;
  /**
   * Whether this is a standard/built-in pack
   */
  is_standard?: boolean;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Pack metadata
   */
  meta?: Record<string, any>;
  /**
   * Unique reference identifier (e.g., "core", "aws", "slack")
   */
  ref: string;
  /**
   * Runtime dependencies (e.g., shell, python, nodejs)
   */
  runtime_deps?: Array<string>;
  /**
   * Tags for categorization
   */
  tags?: Array<string>;
  /**
   * Pack version (semver format recommended)
   */
  version: string;
};
