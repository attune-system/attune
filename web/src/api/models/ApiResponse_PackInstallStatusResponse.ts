/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_PackInstallStatusResponse = {
  /**
   * Response describing a tracked pack installation attempt.
   */
  data: {
    /**
     * Failure detail, when the install failed
     */
    error_message?: string | null;
    /**
     * When the install reached a terminal state
     */
    finished_at?: string | null;
    /**
     * Pack install record id
     */
    install_id: number;
    /**
     * Pack reference this install attempt belongs to
     */
    pack_ref: string;
    /**
     * Pack version being installed
     */
    pack_version: string;
    /**
     * When installation activities started
     */
    started_at: string;
    /**
     * pending, running, activating, succeeded, failed, or rolled_back
     */
    status: string;
    /**
     * ID of the pack_test_execution row produced by the run, when available
     */
    test_execution_id?: number | null;
    /**
     * Snapshot of the PackTestResult, when available
     */
    test_result?: any;
    /**
     * Why the install was triggered (install, update, manual, validation)
     */
    trigger_reason: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
