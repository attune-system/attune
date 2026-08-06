/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for creating a new file-backed artifact version.
 * No file content is included — the caller writes the file directly to
 * `$ATTUNE_ARTIFACTS_DIR/{file_path}` after receiving the response.
 */
export type CreateFileVersionRequest = {
  /**
   * MIME content type (e.g. "text/plain", "application/octet-stream")
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
