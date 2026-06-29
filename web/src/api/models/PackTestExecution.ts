/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { Value } from "./Value";
/**
 * Pack test execution record
 */
export type PackTestExecution = {
  created: string;
  durationMs: number;
  executionTime: string;
  failed: number;
  id: i64;
  packId: i64;
  packVersion: string;
  passRate: number;
  passed: number;
  result: Value;
  skipped: number;
  totalTests: number;
  triggerReason: string;
};
