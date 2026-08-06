/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Environments } from "./Environments";
/**
 * Information about a built environment
 */
export type BuiltEnvironment = {
  /**
   * Build duration in milliseconds
   */
  duration_ms: number;
  /**
   * Built environments
   */
  environments: Environments;
  /**
   * Pack directory path
   */
  pack_path: string;
  /**
   * Pack reference
   */
  pack_ref: string;
};
