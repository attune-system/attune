/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TestSuiteResult } from "./TestSuiteResult";
/**
 * Pack test result structure (not from DB, used for test execution)
 */
export type PackTestResult = {
  durationMs: number;
  executionTime: string;
  failed: number;
  packRef: string;
  packVersion: string;
  passRate: number;
  passed: number;
  skipped: number;
  status: string;
  testSuites: Array<TestSuiteResult>;
  totalTests: number;
};
