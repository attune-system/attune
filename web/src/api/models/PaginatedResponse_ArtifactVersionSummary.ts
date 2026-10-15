/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ArtifactBodyState } from "./ArtifactBodyState";
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_ArtifactVersionSummary = {
  /**
   * The page items
   */
  items: Array<{
    body_state?: null | ArtifactBodyState;
    /**
     * MIME content type
     */
    content_type?: string | null;
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Who created this version
     */
    created_by?: string | null;
    /**
     * Execution that produced this version
     */
    execution?: number | null;
    /**
     * Relative file path for disk-backed versions
     */
    file_path?: string | null;
    /**
     * Version ID
     */
    id: number;
    object_key?: string | null;
    provider_version?: string | null;
    sha256?: string | null;
    /**
     * Size of content in bytes
     */
    size_bytes?: number | null;
    /**
     * Version number
     */
    version: number;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
