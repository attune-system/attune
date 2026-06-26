import { describe, expect, it } from "vitest";
import {
  buildStackedBucketModel,
  toSortedBucketTotals,
} from "./analyticsTransforms";

describe("AnalyticsWidgets deterministic transforms", () => {
  it("produces stable bucket totals regardless of input order", () => {
    const shuffled = [
      { bucket: "2026-06-25T11:00:00Z", label: "failed", value: 2 },
      { bucket: "2026-06-25T10:00:00Z", label: "completed", value: 3 },
      { bucket: "2026-06-25T10:00:00Z", label: "failed", value: 1 },
      { bucket: "2026-06-25T11:00:00Z", label: "completed", value: 4 },
    ];

    const totals = toSortedBucketTotals(shuffled);
    expect(totals).toEqual([
      { bucket: "2026-06-25T10:00:00Z", value: 4 },
      { bucket: "2026-06-25T11:00:00Z", value: 6 },
    ]);
  });

  it("builds stacked model with canonical bucket and label ordering", () => {
    const points = [
      { bucket: "2026-06-25T11:00:00Z", label: "failed", value: 2 },
      { bucket: "2026-06-25T10:00:00Z", label: "timeout", value: 1 },
      { bucket: "2026-06-25T10:00:00Z", label: "completed", value: 3 },
      { bucket: "2026-06-25T11:00:00Z", label: "completed", value: 4 },
    ];

    const model = buildStackedBucketModel(points);

    expect(model.buckets.map((bucket) => bucket.bucket)).toEqual([
      "2026-06-25T10:00:00Z",
      "2026-06-25T11:00:00Z",
    ]);
    expect(model.allLabels).toEqual(["completed", "failed", "timeout"]);
    expect(model.maxTotal).toBe(6);
  });
});

describe.skip("Dashboard data endpoint contract rendering", () => {
  it("renders partial/forbidden/error states distinctly", () => {
    // Blocked: dashboard data endpoint/source envelopes are not implemented yet.
  });
});
