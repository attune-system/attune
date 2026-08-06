/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheNamespacePolicyBody } from "./CacheNamespacePolicyBody";
import type { OwnerType } from "./OwnerType";
/**
 * Update a namespace's publication policy. Owner scope and namespace are
 * immutable; changing either is a new namespace.
 */
export type UpdateCacheNamespaceRequest = CacheNamespacePolicyBody & {
  owner_ref?: string | null;
  owner_type: OwnerType;
};
