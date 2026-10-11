/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheMode } from "./CacheMode";
import type { CacheNamespacePolicyBody } from "./CacheNamespacePolicyBody";
import type { OwnerType } from "./OwnerType";
/**
 * Create a new owner-scoped cache namespace.
 */
export type CreateCacheNamespaceRequest = CacheNamespacePolicyBody & {
  mode?: CacheMode;
  /**
   * Normalized lowercase namespace, e.g. `salesforce.users`.
   */
  namespace: string;
  owner_ref?: string | null;
  owner_type: OwnerType;
};
