/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PackDescriptionPatch } from "./PackDescriptionPatch";
import type { Value } from "./Value";
/**
 * Request DTO for updating a pack
 */
export type UpdatePackRequest = {
  /**
   * Configuration schema
   */
  conf_schema?: Record<string, any> | null;
  /**
   * Pack configuration values
   */
  config?: Record<string, any> | null;
  /**
   * Pack dependencies (refs of required packs)
   */
  dependencies?: Array<string> | null;
  description?: null | PackDescriptionPatch;
  /**
   * Whether this is a standard pack
   */
  is_standard?: boolean | null;
  /**
   * Human-readable label
   */
  label?: string | null;
  /**
   * Pack metadata
   */
  meta?: Record<string, any> | null;
  /**
   * Runtime dependencies (e.g., shell, python, nodejs)
   */
  runtime_deps?: Array<string> | null;
  /**
   * Tags for categorization
   */
  tags?: Array<string> | null;
  /**
   * Pack version
   */
  version?: string | null;
  worker_affinity?: null | Value;
  worker_selector?: null | Value;
  worker_tolerations?: null | Value;
};
