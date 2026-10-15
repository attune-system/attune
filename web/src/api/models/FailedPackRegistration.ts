/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Failed pack registration
 */
export type FailedPackRegistration = {
  details?: null | Value;
  /**
   * Error message
   */
  error: string;
  /**
   * Error stage
   */
  error_stage: string;
  /**
   * Pack path
   */
  pack_path: string;
  /**
   * Pack reference
   */
  pack_ref: string;
};
