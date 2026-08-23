/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PackInstallProvenance } from "./PackInstallProvenance";
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
     * ID of the pack install tracking record, present when tests were dispatched.
     */
    install_id?: number | null;
    /**
     * Current install status: pending, running, activating, succeeded, failed, or rolled_back.
     */
    install_status?: string | null;
    /**
     * The installed/registered pack
     */
    pack: PackResponse;
    provenance?: null | PackInstallProvenance;
    test_result?: null | PackTestResult;
    /**
     * Whether tests were skipped
     */
    tests_skipped: boolean;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
