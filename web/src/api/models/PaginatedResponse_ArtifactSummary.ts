/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ArtifactClassification } from "./ArtifactClassification";
import type { ArtifactType } from "./ArtifactType";
import type { ArtifactVisibility } from "./ArtifactVisibility";
import type { OwnerType } from "./OwnerType";
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_ArtifactSummary = {
  /**
   * The page items
   */
  items: Array<{
    /**
     * Classification used to distinguish runtime log artifacts from general artifacts.
     */
    classification: ArtifactClassification;
    /**
     * MIME content type
     */
    content_type?: string | null;
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Artifact ID
     */
    id: number;
    /**
     * Human-readable name
     */
    name?: string | null;
    /**
     * Owner identifier
     */
    owner: string;
    /**
     * Artifact reference
     */
    ref: string;
    /**
     * Owner scope
     */
    scope: OwnerType;
    /**
     * Size of latest version in bytes
     */
    size_bytes?: number | null;
    /**
     * Artifact type
     */
    type: ArtifactType;
    /**
     * Last update timestamp
     */
    updated: string;
    /**
     * Visibility level
     */
    visibility: ArtifactVisibility;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
