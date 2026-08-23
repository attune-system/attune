/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Response DTO for pack information
 */
export type PackResponse = {
  /**
   * Number of actions registered for this pack
   */
  action_count?: number | null;
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
   * Number of rules registered for this pack
   */
  rule_count?: number | null;
  /**
   * Runtime dependencies (e.g., shell, python, nodejs)
   */
  runtime_deps: Array<string>;
  /**
   * Number of sensors registered for this pack
   */
  sensor_count?: number | null;
  /**
   * Tags
   */
  tags: Array<string>;
  /**
   * Number of triggers registered for this pack
   */
  trigger_count?: number | null;
  /**
   * Last update timestamp
   */
  updated: string;
  /**
   * Pack version
   */
  version: string;
  worker_affinity: Value;
  worker_selector: Value;
  worker_tolerations: Value;
};
