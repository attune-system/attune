/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheNamespacePolicyBody } from "./CacheNamespacePolicyBody";
import type { OwnerType } from "./OwnerType";
/**
 * Create a new owner-scoped cache namespace.
 */
export type CreateCacheNamespaceRequest = CacheNamespacePolicyBody & {
  /**
   * Normalized lowercase namespace, e.g. `salesforce.users`.
   */
  namespace: string;
  owner_ref?: string | null;
  owner_type: OwnerType;
};
