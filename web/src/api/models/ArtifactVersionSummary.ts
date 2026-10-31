/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Simplified version for list endpoints
 */
export type ArtifactVersionSummary = {
  /**
   * MIME content type
   */
  content_type?: string | null;
  /**
   * Creation timestamp
   */
  created: string;
  /**
   * Reserved; arbitrary creator metadata is never exposed by summaries.
   */
  created_by?: string | null;
  /**
   * Execution that produced this version
   */
  execution?: number | null;
  /**
   * Version ID
   */
  id: number;
  /**
   * Size of content in bytes
   */
  size_bytes?: number | null;
  /**
   * Version number
   */
  version: number;
};
