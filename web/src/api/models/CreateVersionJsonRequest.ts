/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for creating a new artifact version with JSON content
 */
export type CreateVersionJsonRequest = {
  /**
   * Structured JSON content for this version
   */
  content: Record<string, any>;
  /**
   * MIME content type override (defaults to "application/json")
   */
  content_type?: string | null;
  /**
   * Who created this version (e.g. action ref, identity, "system")
   */
  created_by?: string | null;
  /**
   * Execution that produced this version (optional)
   */
  execution?: number | null;
  /**
   * Free-form metadata about this version
   */
  meta?: any | null;
};
