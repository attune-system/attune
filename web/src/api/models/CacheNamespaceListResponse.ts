/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheNamespaceResponse } from "./CacheNamespaceResponse";
/**
 * Wrapper for a namespace list scoped to one owner.
 */
export type CacheNamespaceListResponse = {
  namespaces: Array<CacheNamespaceResponse>;
  next_cursor: string | null;
};
