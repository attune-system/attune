/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Effective permissions assigned to an identity.
 *
 * Each entry corresponds to one effective grant and can include optional
 * constraints when the grant is scoped (for example to specific packs or refs).
 */
export type EffectivePermissionResponse = {
  /**
   * Actions allowed for the resource.
   */
  actions: Array<string>;
  /**
   * Optional grant constraints describing permission scope granularity.
   */
  constraints?: any | null;
  /**
   * RBAC resource name.
   */
  resource: string;
};
