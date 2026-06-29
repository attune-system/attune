/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Effective resource-level permissions assigned to an identity.
 */
export type EffectivePermissionResponse = {
  /**
   * Actions allowed for the resource.
   */
  actions: Array<string>;
  /**
   * RBAC resource name.
   */
  resource: string;
};
