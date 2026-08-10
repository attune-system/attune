/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
/**
 * Namespace metadata and freshness/health summary. Never includes entries.
 */
export type CacheNamespaceResponse = {
  active_generation: null | i64;
  /**
   * True when there is no active generation (uninitialized dataset).
   */
  cache_not_populated: boolean;
  created: string;
  /**
   * Stable declarative component ref for a pack-managed namespace.
   */
  definition_ref: string | null;
  freshness_target_seconds: number;
  id: i64;
  /**
   * When the active generation was published.
   */
  last_refreshed_at: string | null;
  /**
   * Whether this namespace is declaratively managed by a pack definition.
   */
  managed: boolean;
  /**
   * Durable ref of the pack that manages this namespace.
   */
  managing_pack_ref: string | null;
  max_generation_bytes: number;
  max_records_per_generation: number;
  max_retained_bytes: number;
  max_retained_generations: number;
  max_staging_generations: number;
  namespace: string;
  /**
   * Canonical owner key (`system` or a numeric owner id as text).
   */
  owner: string;
  /**
   * Owner reference for display, when known.
   */
  owner_ref: string | null;
  owner_type: OwnerType;
  /**
   * Active generation record count, when populated.
   */
  record_count: number | null;
  /**
   * Active generation size in bytes, when populated.
   */
  size_bytes: number | null;
  /**
   * Active generation source revision, when populated.
   */
  source_revision: string | null;
  /**
   * True when the active generation's age exceeds the freshness target.
   */
  stale: boolean;
  /**
   * Whether the namespace is tombstoned and pending bounded cleanup.
   */
  tombstoned: boolean;
  updated: string;
};
