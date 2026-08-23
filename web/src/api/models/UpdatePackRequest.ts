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
  conf_schema: any | null;
  /**
   * Pack configuration values
   */
  config: any | null;
  /**
   * Pack dependencies (refs of required packs)
   */
  dependencies?: any[] | null;
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
  meta: any | null;
  /**
   * Runtime dependencies (e.g., shell, python, nodejs)
   */
  runtime_deps?: any[] | null;
  /**
   * Tags for categorization
   */
  tags?: any[] | null;
  /**
   * Pack version
   */
  version?: string | null;
  worker_affinity?: null | Value;
  worker_selector?: null | Value;
  worker_tolerations?: null | Value;
};
