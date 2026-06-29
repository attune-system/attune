import type { TimeSeriesPoint } from "@/hooks/useAnalytics";

export function aggregateByBucket(
  points: TimeSeriesPoint[],
): Map<string, { total: number; byLabel: Map<string, number> }> {
  const map = new Map<
    string,
    { total: number; byLabel: Map<string, number> }
  >();
  for (const p of points) {
    let entry = map.get(p.bucket);
    if (!entry) {
      entry = { total: 0, byLabel: new Map() };
      map.set(p.bucket, entry);
    }
    entry.total += p.value;
    if (p.label) {
      entry.byLabel.set(p.label, (entry.byLabel.get(p.label) || 0) + p.value);
    }
  }
  return map;
}

export function toSortedBucketTotals(
  points: TimeSeriesPoint[],
): { bucket: string; value: number }[] {
  return Array.from(aggregateByBucket(points).entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([bucket, v]) => ({ bucket, value: v.total }));
}

export function buildStackedBucketModel(points: TimeSeriesPoint[]): {
  buckets: { bucket: string; total: number; byLabel: Map<string, number> }[];
  allLabels: string[];
  maxTotal: number;
} {
  const aggregated = aggregateByBucket(points);
  const sorted = Array.from(aggregated.entries()).sort(([a], [b]) =>
    a.localeCompare(b),
  );

  const labels = new Set<string>();
  sorted.forEach(([, v]) => v.byLabel.forEach((_, k) => labels.add(k)));

  return {
    buckets: sorted.map(([bucket, v]) => ({
      bucket,
      total: v.total,
      byLabel: v.byLabel,
    })),
    allLabels: Array.from(labels).sort(),
    maxTotal: Math.max(1, ...sorted.map(([, v]) => v.total)),
  };
}
