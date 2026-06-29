/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Simplified pack response (for list endpoints)
 */
export type PackSummary = {
  /**
   * Creation timestamp
   */
  created: string;
  /**
   * Pack description
   */
  description?: string | null;
  /**
   * Pack ID
   */
  id: number;
  /**
   * Is standard pack
   */
  is_standard: boolean;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Unique reference identifier
   */
  ref: string;
  /**
   * Tags
   */
  tags: Array<string>;
  /**
   * Last update timestamp
   */
  updated: string;
  /**
   * Pack version
   */
  version: string;
};
