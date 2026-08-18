/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Response describing a tracked pack installation attempt.
 */
export type PackInstallStatusResponse = {
  /**
   * Pack install record id
   */
  installId: number;
  /**
   * Pack reference this install attempt belongs to
   */
  packRef: string;
  /**
   * Pack version being installed
   */
  packVersion: string;
  /**
   * pending, running, succeeded, failed, or rolled_back
   */
  status: string;
  /**
   * Why the install was triggered (install, update, manual, validation)
   */
  triggerReason: string;
  /**
   * ID of the pack_test_execution row produced by the run, when available
   */
  testExecutionId?: null | number;
  /**
   * Snapshot of the PackTestResult, when available
   */
  testResult?: null | Value;
  /**
   * Failure detail, when the install failed
   */
  errorMessage?: null | string;
  /**
   * When installation activities started
   */
  startedAt: string;
  /**
   * When the install reached a terminal state
   */
  finishedAt?: null | string;
};
