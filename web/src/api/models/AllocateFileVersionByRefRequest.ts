/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ArtifactType } from "./ArtifactType";
import type { ArtifactVisibility } from "./ArtifactVisibility";
import type { OwnerType } from "./OwnerType";
import type { RetentionPolicyType } from "./RetentionPolicyType";
/**
 * Request DTO for the upsert-and-allocate endpoint.
 *
 * Looks up an artifact by ref (creating it if it doesn't exist), then
 * allocates a new file-backed version and returns the `file_path` where
 * the caller should write the file on the shared artifact volume.
 *
 * This replaces the multi-step create → 409-handling → allocate dance
 * with a single API call.
 */
export type AllocateFileVersionByRefRequest = {
  /**
   * MIME content type for this version (e.g. "text/plain")
   */
  content_type?: string | null;
  /**
   * Who created this version (e.g. action ref, identity, "system")
   */
  created_by?: string | null;
  /**
   * Optional description
   */
  description?: string | null;
  /**
   * Execution ID to link this artifact to
   */
  execution?: number | null;
  /**
   * Free-form metadata about this version
   */
  meta?: any | null;
  /**
   * Human-readable name
   */
  name?: string | null;
  /**
   * Owner identifier (ref string of the owning entity)
   */
  owner?: string | null;
  /**
   * Retention limit (default: 10)
   */
  retention_limit?: number | null;
  retention_policy?: null | RetentionPolicyType;
  scope?: null | OwnerType;
  type?: null | ArtifactType;
  visibility?: null | ArtifactVisibility;
};
