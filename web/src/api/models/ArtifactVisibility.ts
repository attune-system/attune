/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Visibility level for artifacts.
 * - `Public`: viewable by all authenticated users on the platform.
 * - `Private`: restricted based on the artifact's `scope` and `owner` fields.
 * Full RBAC enforcement is deferred; for now the field enables filtering.
 */
export enum ArtifactVisibility {
  PUBLIC = "public",
  PRIVATE = "private",
}
