/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RbacAction } from "./RbacAction";
import type { RbacGrantConstraints } from "./RbacGrantConstraints";
import type { RbacResource } from "./RbacResource";
export type RbacGrant = {
  actions: Array<RbacAction>;
  constraints?: null | RbacGrantConstraints;
  resource: RbacResource;
};
