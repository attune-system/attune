import type { OwnerType } from "@/api";

export type JsonValue =
  string | number | boolean | null | { [key: string]: JsonValue } | JsonValue[];

/** UI-friendly owner selector converted to generated request arguments by hooks. */
export interface CacheOwnerParams {
  ownerType: OwnerType;
  ownerRef?: string | null;
}

export type CacheNamespaceBrowseScope =
  | { kind: "all" }
  | { kind: "owner"; owner: CacheOwnerParams }
  | { kind: "incomplete" };

/** Machine-readable cache error codes carried in `ApiError.body.code`. */
export enum CacheErrorCode {
  NOT_POPULATED = "cache_not_populated",
  SNAPSHOT_EXPIRED = "snapshot_expired",
  NAMESPACE_DELETED = "namespace_deleted",
  QUOTA_EXCEEDED = "cache_quota_exceeded",
  CONFLICT = "cache_conflict",
  PRECONDITION_FAILED = "cache_precondition_failed",
  STALE = "cache_stale",
  CURSOR_INVALID = "cache_cursor_invalid",
  DUPLICATE_EXTERNAL_ID = "cache_duplicate_external_id",
}
