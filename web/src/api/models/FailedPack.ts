/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Information about a failed pack download
 */
export type FailedPack = {
  details?: null | Value;
  /**
   * Error message
   */
  error: string;
  /**
   * Pack source that failed
   */
  source: string;
};
