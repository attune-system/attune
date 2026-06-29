/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_PackTestSummary = {
  /**
   * The page items
   */
  items: Array<{
    durationMs: number;
    failed: number;
    packId: i64;
    packLabel: string;
    packRef: string;
    packVersion: string;
    passRate: number;
    passed: number;
    skipped: number;
    testExecutionId: i64;
    testTime: string;
    totalTests: number;
    triggerReason: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
