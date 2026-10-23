/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AuthoredGrantConstraints } from "./AuthoredGrantConstraints";
import type { PermissionScope } from "./PermissionScope";
import type { RbacAction } from "./RbacAction";
import type { RbacResource } from "./RbacResource";
/**
 * Canonical authored grant. A grant may override its set's default scope.
 */
export type PermissionGrantDefinition = {
  actions: Array<RbacAction>;
  constraints?: null | AuthoredGrantConstraints;
  resource: RbacResource;
  scope?: null | PermissionScope;
};
