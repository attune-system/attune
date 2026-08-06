import {
  ApiError,
  CacheGenerationState,
  OwnerType,
  type CacheNamespaceResponse,
} from "@/api";
import { CacheErrorCode, type CacheOwnerParams } from "@/types/cache";

const SYSTEM_OWNER_REF_PLACEHOLDER = "_";
const SELF_OWNER_REF_PLACEHOLDER = "self";

/**
 * Derived, UI-facing namespace health used for the index "Status" column.
 * This is not part of the wire contract (`CacheNamespaceResponse` only has
 * the raw `cache_not_populated`/`stale` booleans) — it's computed by
 * `computeNamespaceStatus` below for consistent display across the index and
 * detail pages.
 */
export enum CacheNamespaceStatus {
  UNINITIALIZED = "uninitialized",
  MONITORING_DISABLED = "monitoring_disabled",
  FRESH = "fresh",
  STALE = "stale",
  REFRESHING = "refreshing",
  FAILED = "failed",
}

// ── Formatting ───────────────────────────────────────────────────────────

export function formatDateTime(value?: string | null): string {
  if (!value) return "—";
  return new Date(value).toLocaleString();
}

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) {
    return "—";
  }
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

export function formatRecordCount(count: number | null | undefined): string {
  if (count === null || count === undefined || Number.isNaN(count)) {
    return "—";
  }
  return count.toLocaleString();
}

/** Short single-line JSON preview, truncated so a large record never floods the UI. */
export function formatJsonPreview(value: unknown, maxLength = 120): string {
  let text: string;
  try {
    text = JSON.stringify(value);
  } catch {
    return "(unserializable value)";
  }
  if (text === undefined) return "undefined";
  if (text.length <= maxLength) return text;
  return `${text.slice(0, maxLength - 1)}…`;
}

// ── Owner scope ──────────────────────────────────────────────────────────

const OWNER_TYPE_LABELS: Record<OwnerType, string> = {
  [OwnerType.SYSTEM]: "System",
  [OwnerType.IDENTITY]: "You",
  [OwnerType.PACK]: "Pack",
  [OwnerType.ACTION]: "Action",
  [OwnerType.SENSOR]: "Sensor",
};

export function ownerTypeLabel(ownerType: OwnerType): string {
  return OWNER_TYPE_LABELS[ownerType] ?? ownerType;
}

/** Best available denormalized ref for display, or `null` for system/identity. */
export function ownerDisplayRef(
  namespace: Pick<CacheNamespaceResponse, "owner_type" | "owner_ref" | "owner">,
): string | null {
  if (namespace.owner_type === OwnerType.SYSTEM) return null;
  if (namespace.owner_type === OwnerType.IDENTITY) return null;
  return namespace.owner_ref ?? namespace.owner;
}

export function formatOwnerScope(
  namespace: Pick<CacheNamespaceResponse, "owner_type" | "owner_ref" | "owner">,
): string {
  const ref = ownerDisplayRef(namespace);
  const label = ownerTypeLabel(namespace.owner_type);
  return ref ? `${label}: ${ref}` : label;
}

/** Route/path ref segment consistent with `/caches/:ownerType/:ownerRef/:namespace`. */
export function ownerRefForPath(
  namespace: Pick<CacheNamespaceResponse, "owner_type" | "owner_ref">,
): string {
  switch (namespace.owner_type) {
    case OwnerType.SYSTEM:
      return SYSTEM_OWNER_REF_PLACEHOLDER;
    case OwnerType.IDENTITY:
      return SELF_OWNER_REF_PLACEHOLDER;
    default:
      return namespace.owner_ref ?? "";
  }
}

export function buildCacheNamespacePath(
  ownerType: OwnerType,
  ownerRef: string,
  namespace: string,
): string {
  return `/caches/${encodeURIComponent(ownerType)}/${encodeURIComponent(
    ownerRef || ownerRefPlaceholderFor(ownerType),
  )}/${encodeURIComponent(namespace)}`;
}

function ownerRefPlaceholderFor(ownerType: OwnerType): string {
  if (ownerType === OwnerType.SYSTEM) return SYSTEM_OWNER_REF_PLACEHOLDER;
  if (ownerType === OwnerType.IDENTITY) return SELF_OWNER_REF_PLACEHOLDER;
  return "";
}

/** Parses `:ownerType`/`:ownerRef` route params back into `CacheOwnerParams`. */
export function parseOwnerRouteParams(
  ownerType: string | undefined,
  ownerRef: string | undefined,
): CacheOwnerParams | null {
  const validTypes = Object.values(OwnerType) as string[];
  if (!ownerType || !validTypes.includes(ownerType)) {
    return null;
  }
  const type = ownerType as OwnerType;
  const isPlaceholder =
    ownerRef === SYSTEM_OWNER_REF_PLACEHOLDER ||
    ownerRef === SELF_OWNER_REF_PLACEHOLDER ||
    !ownerRef;
  return {
    ownerType: type,
    ownerRef: isPlaceholder ? null : ownerRef,
  };
}

// ── Records tab bounds ───────────────────────────────────────────────────
//
// Mirror the server-side bounds in crates/common/src/repositories/cache.rs so
// the UI can warn before an oversized request is even sent.
export const MAX_MULTI_LOOKUP_IDS = 1000;
export const MAX_SCAN_PAGE_SIZE = 1000;
export const DEFAULT_SCAN_PAGE_SIZE = 50;

/**
 * Parses a bounded external-ID input (one per line, commas also accepted)
 * into a deduplicated, trimmed list. Returns the parsed IDs plus whether the
 * bounded limit was exceeded (callers should block submission in that case
 * rather than silently truncating the request).
 */
export function parseExternalIdsInput(raw: string): {
  ids: string[];
  exceededLimit: boolean;
} {
  const seen = new Set<string>();
  const ids: string[] = [];
  for (const candidate of raw.split(/[\n,]/)) {
    const trimmed = candidate.trim();
    if (!trimmed || seen.has(trimmed)) continue;
    seen.add(trimmed);
    ids.push(trimmed);
  }
  return {
    ids: ids.slice(0, MAX_MULTI_LOOKUP_IDS),
    exceededLimit: ids.length > MAX_MULTI_LOOKUP_IDS,
  };
}

// ── Namespace naming ─────────────────────────────────────────────────────

// Mirrors the normalized namespace format documented in KEY_CACHE.md:
// lowercase ASCII, starting alphanumeric, `._-` allowed, bounded length.
export const NAMESPACE_NAME_PATTERN = /^[a-z0-9][a-z0-9._-]{0,127}$/;

export function isValidNamespaceName(value: string): boolean {
  return NAMESPACE_NAME_PATTERN.test(value);
}

export const MIN_RETAINED_GENERATIONS = 2;

export function isValidMaxRetainedGenerations(value: number): boolean {
  return Number.isInteger(value) && value >= MIN_RETAINED_GENERATIONS;
}

export function formatFreshnessTarget(seconds: number): string {
  return seconds === 0 ? "Freshness monitoring disabled" : `${seconds}s`;
}

// ── Status / badges ──────────────────────────────────────────────────────

export function computeNamespaceStatus(
  namespace: Pick<
    CacheNamespaceResponse,
    "cache_not_populated" | "freshness_target_seconds" | "stale"
  >,
  hasInProgressRefresh = false,
): CacheNamespaceStatus {
  if (namespace.cache_not_populated) {
    return CacheNamespaceStatus.UNINITIALIZED;
  }
  if (hasInProgressRefresh) {
    return CacheNamespaceStatus.REFRESHING;
  }
  if (namespace.freshness_target_seconds === 0) {
    return CacheNamespaceStatus.MONITORING_DISABLED;
  }
  if (namespace.stale) {
    return CacheNamespaceStatus.STALE;
  }
  return CacheNamespaceStatus.FRESH;
}

const NAMESPACE_STATUS_BADGES: Record<
  CacheNamespaceStatus,
  { label: string; classes: string }
> = {
  [CacheNamespaceStatus.UNINITIALIZED]: {
    label: "Uninitialized",
    classes: "bg-gray-100 text-gray-700",
  },
  [CacheNamespaceStatus.MONITORING_DISABLED]: {
    label: "Freshness disabled",
    classes: "bg-slate-100 text-slate-700",
  },
  [CacheNamespaceStatus.FRESH]: {
    label: "Fresh",
    classes: "bg-green-100 text-green-800",
  },
  [CacheNamespaceStatus.STALE]: {
    label: "Stale",
    classes: "bg-amber-100 text-amber-800",
  },
  [CacheNamespaceStatus.REFRESHING]: {
    label: "Refreshing",
    classes: "bg-blue-100 text-blue-800",
  },
  [CacheNamespaceStatus.FAILED]: {
    label: "Failed",
    classes: "bg-red-100 text-red-800",
  },
};

export function getNamespaceStatusBadge(status: CacheNamespaceStatus) {
  return NAMESPACE_STATUS_BADGES[status];
}

const GENERATION_STATE_BADGES: Record<
  CacheGenerationState,
  { label: string; classes: string }
> = {
  [CacheGenerationState.STAGING]: {
    label: "Staging",
    classes: "bg-blue-100 text-blue-800",
  },
  [CacheGenerationState.READY]: {
    label: "Ready",
    classes: "bg-cyan-100 text-cyan-800",
  },
  [CacheGenerationState.ACTIVE]: {
    label: "Active",
    classes: "bg-green-100 text-green-800",
  },
  [CacheGenerationState.RETIRED]: {
    label: "Retired",
    classes: "bg-gray-100 text-gray-700",
  },
  [CacheGenerationState.FAILED]: {
    label: "Failed",
    classes: "bg-red-100 text-red-800",
  },
};

export function getGenerationStateBadge(state: CacheGenerationState) {
  return GENERATION_STATE_BADGES[state];
}

// ── Error handling ───────────────────────────────────────────────────────
//
// The API's error envelope (`attune_api::middleware::error::ErrorResponse`)
// is `{ error: string (human message), code?: string (machine code), details?
// }`. Cache-specific conditions are always carried in `code` (see
// `CacheErrorCode`), not in `error` — `error` is prose meant for display.

function cacheErrorCode(error: unknown): string | undefined {
  if (!(error instanceof ApiError)) return undefined;
  const body = error.body as { code?: string } | undefined;
  return body?.code;
}

export function getCacheErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError) {
    const body = error.body as { error?: string; message?: string } | undefined;
    return body?.error || body?.message || error.message || fallback;
  }
  if (error instanceof Error) {
    return error.message || fallback;
  }
  return fallback;
}

export function isCacheApiErrorStatus(error: unknown, status: number): boolean {
  return error instanceof ApiError && error.status === status;
}

/** The pinned generation/cursor aged out; never silently falls back to latest. */
export function isSnapshotExpiredError(error: unknown): boolean {
  return cacheErrorCode(error) === CacheErrorCode.SNAPSHOT_EXPIRED;
}

/** The namespace exists but has never had a generation published. */
export function isCacheNotPopulatedError(error: unknown): boolean {
  return cacheErrorCode(error) === CacheErrorCode.NOT_POPULATED;
}

/** A concurrent publish changed the active generation before this promotion. */
export function isPromotionConflictError(error: unknown): boolean {
  return cacheErrorCode(error) === CacheErrorCode.PRECONDITION_FAILED;
}

/** The active generation is past its freshness target and `require_fresh` was set. */
export function isCacheStaleError(error: unknown): boolean {
  return cacheErrorCode(error) === CacheErrorCode.STALE;
}

// ── Bounded NDJSON chunking (browser refresh upload) ────────────────────
//
// These are pure, DOM-free functions so the line-splitting/grouping/checksum
// logic can be unit tested directly. The React component that reads the
// local file is responsible for streaming bounded byte windows through
// `splitCompleteLines` — none of this buffers a whole 200,000-record file in
// memory at once.

export interface NdjsonSplitResult {
  /** Complete, newline-terminated records found in this window. */
  lines: string[];
  /** Trailing partial line to prepend to the next window's text. */
  remainder: string;
}

/** Splits a bounded text window into complete lines plus a carry-over remainder. */
export function splitCompleteLines(
  buffer: string,
  isFinalWindow: boolean,
): NdjsonSplitResult {
  const normalized = buffer.split("\r\n").join("\n");
  const parts = normalized.split("\n");
  if (isFinalWindow) {
    return {
      lines: parts.filter((line) => line.length > 0),
      remainder: "",
    };
  }
  const remainder = parts.pop() ?? "";
  return { lines: parts.filter((line) => line.length > 0), remainder };
}

export interface ParsedCacheIngestRecord {
  external_id: string;
  value: unknown;
  source_updated_at?: string;
  source_checksum?: string;
}

export type ParseNdjsonLineResult =
  | { ok: true; record: ParsedCacheIngestRecord }
  | { ok: false; lineNumber: number; error: string };

/**
 * Parses one NDJSON record line. Error messages intentionally reference only
 * the line number, never the line's content — cache values must never appear
 * in progress logs, toasts, or audit previews, including validation errors.
 */
export function parseNdjsonRecordLine(
  line: string,
  lineNumber: number,
): ParseNdjsonLineResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    return {
      ok: false,
      lineNumber,
      error: `Line ${lineNumber} is not valid JSON.`,
    };
  }

  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return {
      ok: false,
      lineNumber,
      error: `Line ${lineNumber} must be a JSON object.`,
    };
  }

  const record = parsed as Record<string, unknown>;
  if (typeof record.external_id !== "string" || record.external_id === "") {
    return {
      ok: false,
      lineNumber,
      error: `Line ${lineNumber} is missing a non-empty string "external_id".`,
    };
  }
  if (!("value" in record)) {
    return {
      ok: false,
      lineNumber,
      error: `Line ${lineNumber} is missing "value".`,
    };
  }

  return {
    ok: true,
    record: {
      external_id: record.external_id,
      value: record.value,
      source_updated_at:
        typeof record.source_updated_at === "string"
          ? record.source_updated_at
          : undefined,
      source_checksum:
        typeof record.source_checksum === "string"
          ? record.source_checksum
          : undefined,
    },
  };
}

function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

function sumLineBytes(lines: string[]): number {
  return lines.reduce((sum, line) => sum + utf8ByteLength(line) + 1, 0);
}

/**
 * Groups NDJSON lines into bounded chunk batches, respecting both a maximum
 * record count and a maximum approximate byte size per chunk (mirroring the
 * server's `MAX_INGEST_CHUNK_RECORDS` / `MAX_INGEST_CHUNK_BYTES` bounds).
 */
export function groupLinesIntoChunks(
  lines: string[],
  maxRecordsPerChunk: number,
  maxBytesPerChunk: number,
): string[][] {
  const chunks: string[][] = [];
  let current: string[] = [];
  let currentBytes = 0;

  for (const line of lines) {
    const lineBytes = utf8ByteLength(line) + 1; // +1 for the newline separator
    const wouldOverflow =
      current.length >= maxRecordsPerChunk ||
      (current.length > 0 && currentBytes + lineBytes > maxBytesPerChunk);

    if (wouldOverflow) {
      chunks.push(current);
      current = [];
      currentBytes = 0;
    }

    current.push(line);
    currentBytes += lineBytes;
  }

  if (current.length > 0) {
    chunks.push(current);
  }

  return chunks;
}

/**
 * Deterministic client refresh ID derived from the namespace + selected file
 * identity, so reloading the page and re-selecting the same file resumes the
 * same idempotent staging generation via the server's `create_or_get`
 * semantics instead of starting a new one.
 */
export function buildClientRefreshId(
  namespaceKey: string,
  fileName: string,
  fileSize: number,
  fileLastModified: number,
): string {
  return `web:${namespaceKey}:${fileName}:${fileSize}:${fileLastModified}`;
}

// ── Bounded browser file streaming ───────────────────────────────────────
//
// Both helpers below read a local `File`/`Blob` in fixed-size byte windows
// via `File.slice(...).text()` rather than `file.text()`, so a 200,000-record
// (multi-hundred-MB) file is never fully materialized in memory at once —
// only one window and one in-progress chunk's worth of lines are held at a
// time, consistent with the "bounded operation" invariant in KEY_CACHE.md.

const DEFAULT_STREAM_WINDOW_BYTES = 1024 * 1024; // 1 MiB

/** Bounded-memory pass that only counts complete NDJSON lines in a file. */
export async function countRecordsInFile(
  file: Blob,
  windowBytes: number = DEFAULT_STREAM_WINDOW_BYTES,
): Promise<number> {
  let offset = 0;
  let remainder = "";
  let count = 0;

  while (offset < file.size) {
    const end = Math.min(offset + windowBytes, file.size);
    const isFinalWindow = end >= file.size;
    const windowText = await file.slice(offset, end).text();
    const { lines, remainder: nextRemainder } = splitCompleteLines(
      remainder + windowText,
      isFinalWindow,
    );
    count += lines.length;
    remainder = nextRemainder;
    offset = end;
  }

  return count;
}

/**
 * Slices at most one bounded chunk off the front of `lines`, respecting both
 * `maxRecords` and `maxBytes`. Always takes at least one line so a single
 * line longer than `maxBytes` still makes progress instead of stalling.
 */
function takeOneBoundedChunk(
  lines: string[],
  maxRecords: number,
  maxBytes: number,
): { chunk: string[]; rest: string[] } {
  let bytes = 0;
  let count = 0;
  for (const line of lines) {
    const lineBytes = utf8ByteLength(line) + 1; // +1 for the newline separator
    if (count > 0 && (count >= maxRecords || bytes + lineBytes > maxBytes)) {
      break;
    }
    bytes += lineBytes;
    count += 1;
  }
  return { chunk: lines.slice(0, count), rest: lines.slice(count) };
}

/**
 * Streams a file's NDJSON records into bounded chunk batches without ever
 * holding the whole file in memory. Only one text window and one pending
 * (not-yet-bounded-enough-to-emit) line buffer are held at a time. Each
 * yielded batch respects both `maxRecordsPerChunk` and `maxBytesPerChunk`.
 */
export async function* streamFileRecordChunks(
  file: Blob,
  maxRecordsPerChunk: number,
  maxBytesPerChunk: number,
  windowBytes: number = DEFAULT_STREAM_WINDOW_BYTES,
): AsyncGenerator<string[]> {
  let offset = 0;
  let remainder = "";
  let pending: string[] = [];

  while (offset < file.size) {
    const end = Math.min(offset + windowBytes, file.size);
    const isFinalWindow = end >= file.size;
    const windowText = await file.slice(offset, end).text();
    const { lines, remainder: nextRemainder } = splitCompleteLines(
      remainder + windowText,
      isFinalWindow,
    );
    remainder = nextRemainder;
    offset = end;
    pending.push(...lines);

    // Only flush chunks we are sure are complete: keep at least enough lines
    // pending that a byte-bounded chunk boundary can't shift once more lines
    // arrive from the next window. Requiring a full extra chunk's worth of
    // headroom before flushing keeps this simple and still bounded.
    while (
      pending.length > maxRecordsPerChunk ||
      sumLineBytes(pending) > maxBytesPerChunk
    ) {
      const { chunk, rest } = takeOneBoundedChunk(
        pending,
        maxRecordsPerChunk,
        maxBytesPerChunk,
      );
      yield chunk;
      pending = rest;
    }
  }

  while (pending.length > 0) {
    const { chunk, rest } = takeOneBoundedChunk(
      pending,
      maxRecordsPerChunk,
      maxBytesPerChunk,
    );
    yield chunk;
    pending = rest;
  }
}
