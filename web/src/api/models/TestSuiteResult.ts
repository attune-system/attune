/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TestCaseResult } from "./TestCaseResult";
/**
 * Test suite result (collection of test cases)
 */
export type TestSuiteResult = {
  durationMs: number;
  failed: number;
  name: string;
  passed: number;
  runnerType: string;
  skipped: number;
  testCases: Array<TestCaseResult>;
  total: number;
};
