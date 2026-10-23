/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ArtifactVisibility } from "./ArtifactVisibility";
import type { OwnerType } from "./OwnerType";
import type { RbacExecutionScopeConstraint } from "./RbacExecutionScopeConstraint";
import type { RbacOwnerConstraint } from "./RbacOwnerConstraint";
import type { RbacPackOwnerConstraint } from "./RbacPackOwnerConstraint";
import type { RbacPackStateConstraint } from "./RbacPackStateConstraint";
/**
 * Public authoring schema. Structural ownership and database target IDs are
 * server-owned and deliberately absent. Runtime validation remains canonical.
 */
export type AuthoredGrantConstraints = {
  attributes?: Record<string, any> | null;
  encrypted?: boolean | null;
  execution_scope?: null | RbacExecutionScopeConstraint;
  owner?: null | RbacOwnerConstraint;
  owner_refs?: Array<string> | null;
  owner_types?: Array<OwnerType> | null;
  pack_owner?: null | RbacPackOwnerConstraint;
  pack_refs?: Array<string> | null;
  pack_state?: null | RbacPackStateConstraint;
  refs?: Array<string> | null;
  visibility?: Array<ArtifactVisibility> | null;
};
