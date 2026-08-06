/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ArtifactJsonPatch } from "./ArtifactJsonPatch";
import type { ArtifactStringPatch } from "./ArtifactStringPatch";
import type { ArtifactType } from "./ArtifactType";
import type { ArtifactVisibility } from "./ArtifactVisibility";
import type { OwnerType } from "./OwnerType";
import type { RetentionPolicyType } from "./RetentionPolicyType";
/**
 * Request DTO for updating an existing artifact
 */
export type UpdateArtifactRequest = {
  content_type?: null | ArtifactStringPatch;
  data?: null | ArtifactJsonPatch;
  description?: null | ArtifactStringPatch;
  name?: null | ArtifactStringPatch;
  /**
   * Updated owner identifier
   */
  owner?: string | null;
  /**
   * Updated retention limit
   */
  retention_limit?: number | null;
  retention_policy?: null | RetentionPolicyType;
  scope?: null | OwnerType;
  type?: null | ArtifactType;
  visibility?: null | ArtifactVisibility;
};
