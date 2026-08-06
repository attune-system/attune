/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ArtifactType } from "./ArtifactType";
import type { ArtifactVisibility } from "./ArtifactVisibility";
import type { OwnerType } from "./OwnerType";
import type { RetentionPolicyType } from "./RetentionPolicyType";
/**
 * Request DTO for creating a new artifact
 */
export type CreateArtifactRequest = {
  /**
   * MIME content type (e.g. "text/plain", "application/json")
   */
  content_type?: string | null;
  /**
   * Initial structured data (for progress-type artifacts or metadata)
   */
  data?: any | null;
  /**
   * Optional description
   */
  description?: string | null;
  /**
   * Human-readable name
   */
  name?: string | null;
  /**
   * Owner identifier (ref string of the owning entity)
   */
  owner: string;
  /**
   * Artifact reference (unique identifier, e.g. "build.log", "test.results")
   */
  ref: string;
  /**
   * Retention limit (number of versions, days, hours, or minutes depending on policy).
   * If omitted, execution/action/sensor defaults may apply.
   */
  retention_limit?: number | null;
  retention_policy?: null | RetentionPolicyType;
  /**
   * Owner scope type
   */
  scope: OwnerType;
  /**
   * Artifact type
   */
  type: ArtifactType;
  visibility?: null | ArtifactVisibility;
};
