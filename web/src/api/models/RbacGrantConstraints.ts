/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ArtifactVisibility } from "./ArtifactVisibility";
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
import type { RbacExecutionScopeConstraint } from "./RbacExecutionScopeConstraint";
import type { RbacOwnerConstraint } from "./RbacOwnerConstraint";
import type { RbacPackOwnerConstraint } from "./RbacPackOwnerConstraint";
import type { RbacPackStateConstraint } from "./RbacPackStateConstraint";
import type { Value } from "./Value";
export type RbacGrantConstraints = {
  attributes?: Record<string, Value> | null;
  encrypted?: boolean | null;
  execution_scope?: null | RbacExecutionScopeConstraint;
  ids?: Array<i64> | null;
  managing_pack_id?: null | i64;
  owner?: null | RbacOwnerConstraint;
  owner_refs?: Array<string> | null;
  owner_types?: Array<OwnerType> | null;
  pack_owner?: null | RbacPackOwnerConstraint;
  pack_refs?: Array<string> | null;
  pack_state?: null | RbacPackStateConstraint;
  refs?: Array<string> | null;
  visibility?: Array<ArtifactVisibility> | null;
};
