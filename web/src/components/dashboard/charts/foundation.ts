import { format as d3Format, scaleOrdinal } from "d3";
import type { DashboardSourceResult } from "@/types/dashboard";

export type ChartRow = Record<string, unknown>;

export const CHART_PALETTE = [
  "#2563eb",
  "#059669",
  "#dc2626",
  "#7c3aed",
  "#ea580c",
  "#0891b2",
  "#4f46e5",
  "#65a30d",
  "#db2777",
  "#0f766e",
];

const LEVEL_COLORS: Record<string, string> = {
  good: "#16a34a",
  warning: "#f59e0b",
  bad: "#dc2626",
  ok: "#16a34a",
  error: "#dc2626",
};

export function toRows(data: DashboardSourceResult["data"]): ChartRow[] {
  if (!data) return [];
  if (Array.isArray(data)) return data;
  return [data];
}

export function asNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  return null;
}

export function toKey(value: unknown, fallback = "value"): string {
  if (value === null || value === undefined) return fallback;
  return String(value);
}

function formatDurationMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60000) return `${d3Format(".1f")(ms / 1000)} s`;
  if (ms < 3600000) return `${d3Format(".1f")(ms / 60000)} min`;
  return `${d3Format(".1f")(ms / 3600000)} h`;
}

export function formatValue(
  value: unknown,
  formatHint?: string,
  unitHint?: string,
): string {
  if (value === null || value === undefined) return "—";

  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }

  const numeric = asNumber(value);
  const format = unitHint || formatHint;
  if (numeric !== null) {
    if (format === "percent") {
      return `${d3Format(".1f")(numeric)}%`;
    }
    if (format === "duration_ms") {
      return formatDurationMs(numeric);
    }
    if (format === "float") {
      return d3Format(",.2f")(numeric);
    }
    if (format === "integer" || format === "count") {
      return d3Format(",.0f")(numeric);
    }
    return d3Format(",.2~f")(numeric);
  }

  if (format === "relative_time" && typeof value === "string") {
    const date = new Date(value);
    if (!Number.isNaN(date.valueOf())) {
      return date.toLocaleString();
    }
  }

  if (typeof value === "string") return value;

  return JSON.stringify(value);
}

export function getLevelColor(level: string | undefined, fallback = "#6b7280"): string {
  if (!level) return fallback;
  return LEVEL_COLORS[level] || fallback;
}

export interface CartesianSeriesModel {
  xDomain: string[];
  seriesKeys: string[];
  valuesBySeries: Map<string, Array<number | null>>;
  maxY: number;
}

export function buildCartesianSeriesModel(
  rows: ChartRow[],
  xField: string,
  yField: string,
  seriesField?: string,
): CartesianSeriesModel {
  const xDomain: string[] = [];
  const xIndex = new Map<string, number>();
  const seriesKeys: string[] = [];
  const seriesIndex = new Map<string, number>();
  const valuesBySeries = new Map<string, Array<number | null>>();

  for (const row of rows) {
    const xRaw = row[xField];
    if (xRaw === undefined || xRaw === null) continue;
    const x = toKey(xRaw);

    if (!xIndex.has(x)) {
      xIndex.set(x, xDomain.length);
      xDomain.push(x);
      for (const values of valuesBySeries.values()) {
        values.push(null);
      }
    }

    const seriesKey = seriesField ? toKey(row[seriesField]) : "value";
    if (!seriesIndex.has(seriesKey)) {
      seriesIndex.set(seriesKey, seriesKeys.length);
      seriesKeys.push(seriesKey);
      valuesBySeries.set(seriesKey, new Array(xDomain.length).fill(null));
    }

    const values = valuesBySeries.get(seriesKey);
    const xi = xIndex.get(x);
    const y = asNumber(row[yField]);
    if (!values || xi === undefined || y === null) continue;

    while (values.length < xDomain.length) {
      values.push(null);
    }
    values[xi] = values[xi] === null ? y : (values[xi] ?? 0) + y;
  }

  const maxY = Math.max(
    1,
    ...Array.from(valuesBySeries.values()).flatMap((series) =>
      series.filter((value): value is number => value !== null),
    ),
  );

  return {
    xDomain,
    seriesKeys,
    valuesBySeries,
    maxY,
  };
}

export function createSeriesColorScale(seriesKeys: string[]) {
  return scaleOrdinal<string, string>()
    .domain(seriesKeys)
    .range(CHART_PALETTE);
}

export function pickPreferredColumns(rows: ChartRow[], ordering: string[]): string[] {
  const seen = new Set<string>();
  const columns: string[] = [];

  for (const col of ordering) {
    if (!seen.has(col) && rows.some((row) => col in row)) {
      seen.add(col);
      columns.push(col);
    }
  }

  for (const row of rows) {
    for (const key of Object.keys(row)) {
      if (!seen.has(key)) {
        seen.add(key);
        columns.push(key);
      }
    }
  }

  return columns;
}
