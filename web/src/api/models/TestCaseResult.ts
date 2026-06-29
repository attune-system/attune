/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TestStatus } from "./TestStatus";
/**
 * Individual test case result
 */
export type TestCaseResult = {
  durationMs: number;
  errorMessage?: string | null;
  name: string;
  status: TestStatus;
  stderr?: string | null;
  stdout?: string | null;
};
