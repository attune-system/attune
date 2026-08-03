import { describe, expect, it } from "vitest";
import { CacheGenerationState, OwnerType } from "@/api/cache";
import {
  buildCacheNamespacePath,
  buildClientRefreshId,
  CacheNamespaceStatus,
  computeNamespaceStatus,
  countRecordsInFile,
  formatBytes,
  formatFreshnessTarget,
  formatJsonPreview,
  formatOwnerScope,
  formatRecordCount,
  getGenerationStateBadge,
  getNamespaceStatusBadge,
  groupLinesIntoChunks,
  isCacheNotPopulatedError,
  isValidMaxRetainedGenerations,
  isPromotionConflictError,
  isSnapshotExpiredError,
  isValidNamespaceName,
  ownerRefForPath,
  parseExternalIdsInput,
  parseNdjsonRecordLine,
  parseOwnerRouteParams,
  splitCompleteLines,
  streamFileRecordChunks,
} from "@/components/caches/cacheUtils";
import { ApiError } from "@/api/core/ApiError";

function makeApiError(status: number, body: unknown): ApiError {
  return new ApiError(
    { method: "GET", url: "/x" },
    { url: "/x", ok: false, status, statusText: "err", body },
    "error",
  );
}

describe("formatBytes", () => {
  it("renders sub-kilobyte sizes in bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
  });

  it("renders larger sizes with the right unit", () => {
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });

  it("handles missing values", () => {
    expect(formatBytes(null)).toBe("—");
    expect(formatBytes(undefined)).toBe("—");
  });
});

describe("formatRecordCount", () => {
  it("adds thousands separators", () => {
    expect(formatRecordCount(1234567)).toBe("1,234,567");
  });

  it("handles missing values", () => {
    expect(formatRecordCount(null)).toBe("—");
  });
});

describe("formatJsonPreview", () => {
  it("returns short values unchanged", () => {
    expect(formatJsonPreview({ a: 1 })).toBe('{"a":1}');
  });

  it("truncates long values", () => {
    const long = { text: "x".repeat(200) };
    const preview = formatJsonPreview(long, 50);
    expect(preview.length).toBe(50);
    expect(preview.endsWith("…")).toBe(true);
  });
});

describe("owner scope formatting", () => {
  const baseNamespace = {
    owner_type: OwnerType.PACK,
    owner_ref: "salesforce",
    owner: "42",
  };

  it("prefers the denormalized owner ref for display", () => {
    expect(formatOwnerScope(baseNamespace)).toBe("Pack: salesforce");
  });

  it("falls back to the canonical owner key when no ref is denormalized", () => {
    expect(formatOwnerScope({ ...baseNamespace, owner_ref: null })).toBe(
      "Pack: 42",
    );
  });

  it("shows only the label for system/identity scopes", () => {
    expect(
      formatOwnerScope({
        owner_type: OwnerType.SYSTEM,
        owner_ref: null,
        owner: "system",
      }),
    ).toBe("System");
  });

  it("derives the path ref segment per owner type", () => {
    expect(
      ownerRefForPath({ owner_type: OwnerType.SYSTEM, owner_ref: null }),
    ).toBe("_");
    expect(
      ownerRefForPath({ owner_type: OwnerType.SENSOR, owner_ref: "my-sensor" }),
    ).toBe("my-sensor");
  });

  it("builds and parses the /caches/:ownerType/:ownerRef/:namespace route consistently", () => {
    const path = buildCacheNamespacePath(OwnerType.PACK, "salesforce", "users");
    expect(path).toBe("/caches/pack/salesforce/users");

    const parsed = parseOwnerRouteParams("pack", "salesforce");
    expect(parsed).toEqual({
      ownerType: OwnerType.PACK,
      ownerRef: "salesforce",
    });
  });

  it("parses system/identity placeholders back to a null owner ref", () => {
    expect(parseOwnerRouteParams("system", "_")).toEqual({
      ownerType: OwnerType.SYSTEM,
      ownerRef: null,
    });
    expect(parseOwnerRouteParams("identity", "self")).toEqual({
      ownerType: OwnerType.IDENTITY,
      ownerRef: null,
    });
  });

  it("rejects an unrecognized owner type", () => {
    expect(parseOwnerRouteParams("bogus", "x")).toBeNull();
    expect(parseOwnerRouteParams(undefined, undefined)).toBeNull();
  });
});

describe("isValidNamespaceName", () => {
  it("accepts lowercase alphanumeric names with . _ -", () => {
    expect(isValidNamespaceName("users")).toBe(true);
    expect(isValidNamespaceName("users.v2")).toBe(true);
    expect(isValidNamespaceName("cost_centers-2026")).toBe(true);
  });

  it("rejects uppercase, empty, or invalid leading characters", () => {
    expect(isValidNamespaceName("Users")).toBe(false);
    expect(isValidNamespaceName("")).toBe(false);
    expect(isValidNamespaceName(".users")).toBe(false);
    expect(isValidNamespaceName("_users")).toBe(false);
  });

  it("rejects names over 128 characters", () => {
    expect(isValidNamespaceName("a".repeat(129))).toBe(false);
    expect(isValidNamespaceName("a".repeat(128))).toBe(true);
  });
});

describe("computeNamespaceStatus", () => {
  const freshness_target_seconds = 3600;

  it("is uninitialized without an active generation", () => {
    expect(
      computeNamespaceStatus({
        cache_not_populated: true,
        freshness_target_seconds,
        stale: false,
      }),
    ).toBe(CacheNamespaceStatus.UNINITIALIZED);
  });

  it("is fresh when active and not stale", () => {
    expect(
      computeNamespaceStatus({
        cache_not_populated: false,
        freshness_target_seconds,
        stale: false,
      }),
    ).toBe(CacheNamespaceStatus.FRESH);
  });

  it("is stale when past the freshness target", () => {
    expect(
      computeNamespaceStatus({
        cache_not_populated: false,
        freshness_target_seconds,
        stale: true,
      }),
    ).toBe(CacheNamespaceStatus.STALE);
  });

  it("is refreshing when a refresh is in progress, even if also stale", () => {
    expect(
      computeNamespaceStatus(
        {
          cache_not_populated: false,
          freshness_target_seconds,
          stale: true,
        },
        true,
      ),
    ).toBe(CacheNamespaceStatus.REFRESHING);
  });

  it("does not display a namespace as stale when freshness monitoring is disabled", () => {
    expect(
      computeNamespaceStatus({
        cache_not_populated: false,
        freshness_target_seconds: 0,
        stale: true,
      }),
    ).toBe(CacheNamespaceStatus.MONITORING_DISABLED);
    expect(formatFreshnessTarget(0)).toBe("Freshness monitoring disabled");
  });
});

describe("cache policy validation", () => {
  it("requires at least two retained generations", () => {
    expect(isValidMaxRetainedGenerations(1)).toBe(false);
    expect(isValidMaxRetainedGenerations(2)).toBe(true);
    expect(isValidMaxRetainedGenerations(2.5)).toBe(false);
  });
});

describe("status/state badges", () => {
  it("has a badge for every namespace status", () => {
    for (const status of Object.values(CacheNamespaceStatus)) {
      expect(getNamespaceStatusBadge(status).label).toBeTruthy();
    }
  });

  it("has a badge for every generation state", () => {
    for (const state of Object.values(CacheGenerationState)) {
      expect(getGenerationStateBadge(state).label).toBeTruthy();
    }
  });
});

describe("cache error classification", () => {
  // The API's error envelope carries the machine-readable discriminator in
  // `code`, not `error` (which is a human-readable message) — see
  // attune_api::middleware::error::ErrorResponse and
  // crates/api/src/routes/cache.rs::CacheApiError.
  it("recognizes a snapshot-expired error by its code, not by status alone", () => {
    expect(
      isSnapshotExpiredError(makeApiError(409, { code: "snapshot_expired" })),
    ).toBe(true);
    expect(
      isSnapshotExpiredError(makeApiError(409, { code: "cache_stale" })),
    ).toBe(false);
    expect(isSnapshotExpiredError(makeApiError(404, {}))).toBe(false);
    expect(isSnapshotExpiredError(new Error("network error"))).toBe(false);
  });

  it("recognizes a cache_not_populated error", () => {
    expect(
      isCacheNotPopulatedError(
        makeApiError(409, { code: "cache_not_populated" }),
      ),
    ).toBe(true);
    expect(isCacheNotPopulatedError(makeApiError(409, {}))).toBe(false);
  });

  it("recognizes a promotion precondition conflict distinctly from a generic conflict", () => {
    expect(
      isPromotionConflictError(
        makeApiError(409, { code: "cache_precondition_failed" }),
      ),
    ).toBe(true);
    expect(
      isPromotionConflictError(makeApiError(409, { code: "cache_conflict" })),
    ).toBe(false);
  });
});

describe("parseExternalIdsInput", () => {
  it("splits on newlines and commas, trims, and dedupes", () => {
    const { ids, exceededLimit } = parseExternalIdsInput("a\nb, c\n\n a \n b");
    expect(ids).toEqual(["a", "b", "c"]);
    expect(exceededLimit).toBe(false);
  });

  it("bounds the result and reports when the limit was exceeded", () => {
    const raw = Array.from({ length: 1500 }, (_, i) => `id-${i}`).join("\n");
    const { ids, exceededLimit } = parseExternalIdsInput(raw);
    expect(ids.length).toBe(1000);
    expect(exceededLimit).toBe(true);
  });
});

describe("splitCompleteLines", () => {
  it("keeps a trailing partial line as remainder when not final", () => {
    const result = splitCompleteLines('{"a":1}\n{"a":2}\n{"a":3', false);
    expect(result.lines).toEqual(['{"a":1}', '{"a":2}']);
    expect(result.remainder).toBe('{"a":3');
  });

  it("includes the trailing partial line as complete on the final window", () => {
    const result = splitCompleteLines('{"a":1}\n{"a":3', true);
    expect(result.lines).toEqual(['{"a":1}', '{"a":3']);
    expect(result.remainder).toBe("");
  });

  it("normalizes CRLF line endings", () => {
    const result = splitCompleteLines("a\r\nb\r\n", true);
    expect(result.lines).toEqual(["a", "b"]);
  });
});

describe("parseNdjsonRecordLine", () => {
  it("parses a well-formed record", () => {
    const result = parseNdjsonRecordLine(
      '{"external_id":"id-1","value":{"name":"Ada"}}',
      1,
    );
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.record.external_id).toBe("id-1");
      expect(result.record.value).toEqual({ name: "Ada" });
    }
  });

  it("rejects invalid JSON without echoing the line content", () => {
    const result = parseNdjsonRecordLine("not json", 7);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.lineNumber).toBe(7);
      expect(result.error).toBe("Line 7 is not valid JSON.");
      expect(result.error).not.toContain("not json");
    }
  });

  it("rejects a missing external_id", () => {
    const result = parseNdjsonRecordLine('{"value":1}', 2);
    expect(result.ok).toBe(false);
  });

  it("rejects a missing value", () => {
    const result = parseNdjsonRecordLine('{"external_id":"id-1"}', 3);
    expect(result.ok).toBe(false);
  });
});

describe("groupLinesIntoChunks", () => {
  it("bounds by record count", () => {
    const lines = Array.from({ length: 10 }, (_, i) => `line-${i}`);
    const chunks = groupLinesIntoChunks(lines, 3, 1_000_000);
    expect(chunks.map((c) => c.length)).toEqual([3, 3, 3, 1]);
  });

  it("bounds by byte size", () => {
    const lines = ["a".repeat(10), "b".repeat(10), "c".repeat(10)];
    // Each line + newline is 11 bytes; cap at 15 bytes forces one per chunk.
    const chunks = groupLinesIntoChunks(lines, 100, 15);
    expect(chunks).toEqual([[lines[0]], [lines[1]], [lines[2]]]);
  });
});

describe("buildClientRefreshId", () => {
  it("is deterministic for the same namespace + file identity", () => {
    const id1 = buildClientRefreshId(
      "pack:salesforce:users",
      "a.ndjson",
      100,
      123,
    );
    const id2 = buildClientRefreshId(
      "pack:salesforce:users",
      "a.ndjson",
      100,
      123,
    );
    expect(id1).toBe(id2);
  });

  it("differs when the file identity differs", () => {
    const id1 = buildClientRefreshId(
      "pack:salesforce:users",
      "a.ndjson",
      100,
      123,
    );
    const id2 = buildClientRefreshId(
      "pack:salesforce:users",
      "a.ndjson",
      101,
      123,
    );
    expect(id1).not.toBe(id2);
  });
});

describe("bounded file streaming", () => {
  function ndjsonFile(recordCount: number): Blob {
    const lines: string[] = [];
    for (let i = 0; i < recordCount; i += 1) {
      lines.push(JSON.stringify({ external_id: `id-${i}`, value: i }));
    }
    return new Blob([`${lines.join("\n")}\n`], {
      type: "application/x-ndjson",
    });
  }

  it("counts records without loading the whole file at once", async () => {
    const file = ndjsonFile(250);
    // A tiny window forces many bounded read passes.
    const count = await countRecordsInFile(file, 64);
    expect(count).toBe(250);
  });

  it("streams records into bounded chunk batches that reconstruct the input", async () => {
    const file = ndjsonFile(37);
    const chunks: string[][] = [];
    for await (const chunk of streamFileRecordChunks(file, 5, 1_000_000, 64)) {
      expect(chunk.length).toBeLessThanOrEqual(5);
      chunks.push(chunk);
    }

    const allLines = chunks.flat();
    expect(allLines.length).toBe(37);
    // Every yielded chunk (but the last) is exactly the requested size.
    for (const chunk of chunks.slice(0, -1)) {
      expect(chunk.length).toBe(5);
    }
    // Records are emitted in file order.
    expect(JSON.parse(allLines[0]).external_id).toBe("id-0");
    expect(JSON.parse(allLines[36]).external_id).toBe("id-36");
  });

  it("respects a small byte bound even with a large record-count bound", async () => {
    const file = ndjsonFile(20);
    const chunks: string[][] = [];
    for await (const chunk of streamFileRecordChunks(file, 1000, 40, 4096)) {
      chunks.push(chunk);
    }
    // With ~15-16 bytes/line and a 40-byte cap, no chunk should hold more
    // than a handful of records.
    for (const chunk of chunks) {
      const bytes = chunk.reduce((sum, line) => sum + line.length + 1, 0);
      expect(bytes).toBeLessThanOrEqual(40 + 20); // small slack for boundary line
    }
    expect(chunks.flat().length).toBe(20);
  });
});
