/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheGenerationResponse } from "./CacheGenerationResponse";
/**
 * Wrapper for a generation list.
 */
export type CacheGenerationListResponse = {
  generations: Array<CacheGenerationResponse>;
  next_cursor: string | null;
};
