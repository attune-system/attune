/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PackResponse } from "./PackResponse";
import type { PackTestResult } from "./PackTestResult";
/**
 * Response for pack install/register operations with test results
 */
export type PackInstallResponse = {
  /**
   * The installed/registered pack
   */
  pack: PackResponse;
  test_result?: null | PackTestResult;
  /**
   * Whether tests were skipped
   */
  tests_skipped: boolean;
};
