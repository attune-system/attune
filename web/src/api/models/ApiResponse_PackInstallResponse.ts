/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PackResponse } from "./PackResponse";
import type { PackTestResult } from "./PackTestResult";
/**
 * Standard API response wrapper
 */
export type ApiResponse_PackInstallResponse = {
  /**
   * Response for pack install/register operations with test results
   */
  data: {
    /**
     * The installed/registered pack
     */
    pack: PackResponse;
    test_result?: null | PackTestResult;
    /**
     * Whether tests were skipped
     */
    tests_skipped: boolean;
    /**
     * ID of the pack install tracking record, present when tests were dispatched
     */
    install_id?: null | number;
    /**
     * Current install status: pending, running, succeeded, failed, or rolled_back
     */
    install_status?: null | string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
