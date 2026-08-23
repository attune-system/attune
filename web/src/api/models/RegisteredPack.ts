/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ComponentCounts } from "./ComponentCounts";
import type { TestResult } from "./TestResult";
import type { ValidationResults } from "./ValidationResults";
/**
 * Information about a registered pack
 */
export type RegisteredPack = {
  /**
   * Registered components by type
   */
  components_registered: ComponentCounts;
  /**
   * Pack install tracking ID when tests were dispatched or failed to dispatch
   */
  install_id?: number | null;
  /**
   * Current pack install status
   */
  install_status?: string | null;
  /**
   * Pack database ID
   */
  pack_id: number;
  /**
   * Pack reference
   */
  pack_ref: string;
  /**
   * Pack version
   */
  pack_version: string;
  /**
   * Permanent storage path
   */
  storage_path: string;
  test_result?: null | TestResult;
  /**
   * Validation results
   */
  validation_results: ValidationResults;
};
