import { useMemo, useState } from "react";
import {
  Activity,
  AlertTriangle,
  BarChart3,
  CheckCircle,
  Server,
  Zap,
} from "lucide-react";
import type {
  DashboardAnalytics,
  TimeSeriesPoint,
  FailureRateSummary,
} from "@/hooks/useAnalytics";
import {
  buildStackedBucketModel,
  toSortedBucketTotals,
} from "./analyticsTransforms";

// ---------------------------------------------------------------------------
// Shared types & helpers
// ---------------------------------------------------------------------------

type TimeRangeHours = 6 | 12 | 24 | 48 | 168;

const TIME_RANGE_OPTIONS: { label: string; value: TimeRangeHours }[] = [
  { label: "6h", value: 6 },
  { label: "12h", value: 12 },
  { label: "24h", value: 24 },
  { label: "2d", value: 48 },
  { label: "7d", value: 168 },
];

function formatBucketLabel(iso: string, rangeHours: number): string {
  const d = new Date(iso);
  if (rangeHours <= 24) {
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  if (rangeHours <= 48) {
    return d.toLocaleDateString([], { weekday: "short", hour: "2-digit" });
  }
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function formatBucketTooltip(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString();
}

/**
 * Aggregate TimeSeriesPoints into per-bucket totals or per-bucket-per-label groups.
 */
// ---------------------------------------------------------------------------
// TimeRangeSelector
// ---------------------------------------------------------------------------

interface TimeRangeSelectorProps {
  value: TimeRangeHours;
  onChange: (v: TimeRangeHours) => void;
}

function TimeRangeSelector({ value, onChange }: TimeRangeSelectorProps) {
  return (
    <div className="inline-flex items-center bg-gray-100 rounded-md p-0.5 text-xs">
      {TIME_RANGE_OPTIONS.map((opt) => (
        <button
          key={opt.value}
          onClick={() => onChange(opt.value)}
          className={`px-2 py-1 rounded transition-colors ${
            value === opt.value
              ? "bg-white shadow text-gray-900 font-medium"
              : "text-gray-500 hover:text-gray-700"
          }`}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// MiniBarChart — pure-CSS bar chart for time-series data
// ---------------------------------------------------------------------------

interface MiniBarChartProps {
  /** Ordered time buckets with totals */
  buckets: { bucket: string; value: number }[];
  /** Current time range in hours (affects label formatting) */
  rangeHours: number;
  /** Bar color class (Tailwind bg-* class) */
  barColor?: string;
  /** Height of the chart in pixels */
  height?: number;
  /** Show zero line */
  showZeroLine?: boolean;
}

function MiniBarChart({
  buckets,
  rangeHours,
  barColor = "bg-blue-500",
  height = 120,
  showZeroLine = true,
}: MiniBarChartProps) {
  const [hoveredIdx, setHoveredIdx] = useState<number | null>(null);

  const maxValue = useMemo(
    () => Math.max(1, ...buckets.map((b) => b.value)),
    [buckets],
  );

  if (buckets.length === 0) {
    return (
      <div
        className="flex items-center justify-center text-gray-400 text-xs"
        style={{ height }}
      >
        No data in this time range
      </div>
    );
  }

  // For large ranges, show fewer labels to avoid clutter
  const labelEvery =
    buckets.length > 24
      ? Math.ceil(buckets.length / 8)
      : buckets.length > 12
        ? 2
        : 1;

  return (
    <div className="relative" style={{ height: height + 24 }}>
      {/* Tooltip */}
      {hoveredIdx !== null && buckets[hoveredIdx] && (
        <div className="absolute -top-1 left-1/2 -translate-x-1/2 z-10 bg-gray-800 text-white text-xs rounded px-2 py-1 whitespace-nowrap pointer-events-none shadow-lg">
          {formatBucketTooltip(buckets[hoveredIdx].bucket)}:{" "}
          <span className="font-semibold">{buckets[hoveredIdx].value}</span>
        </div>
      )}

      {/* Bars */}
      <div className="flex items-end gap-px w-full" style={{ height }}>
        {buckets.map((b, i) => {
          const pct = (b.value / maxValue) * 100;
          return (
            <div
              key={b.bucket}
              className="flex-1 min-w-0 relative group"
              style={{ height: "100%" }}
              onMouseEnter={() => setHoveredIdx(i)}
              onMouseLeave={() => setHoveredIdx(null)}
            >
              <div className="absolute bottom-0 inset-x-0 flex justify-center">
                <div
                  className={`w-full rounded-t-sm transition-all duration-150 ${
                    hoveredIdx === i ? barColor.replace("500", "600") : barColor
                  } ${hoveredIdx === i ? "opacity-100" : "opacity-80"}`}
                  style={{
                    height: `${Math.max(pct, b.value > 0 ? 2 : 0)}%`,
                    minHeight: b.value > 0 ? "2px" : "0",
                  }}
                />
              </div>
            </div>
          );
        })}
      </div>

      {/* Zero line */}
      {showZeroLine && (
        <div className="absolute bottom-6 left-0 right-0 border-t border-gray-200" />
      )}

      {/* X-axis labels */}
      <div className="flex items-start mt-1 h-5">
        {buckets.map((b, i) =>
          i % labelEvery === 0 ? (
            <div
              key={b.bucket}
              className="flex-1 text-center text-[9px] text-gray-400 truncate"
              style={{ minWidth: 0 }}
            >
              {formatBucketLabel(b.bucket, rangeHours)}
            </div>
          ) : (
            <div key={b.bucket} className="flex-1" />
          ),
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// StackedBarChart — stacked bar chart for status breakdowns
// ---------------------------------------------------------------------------

const STATUS_COLORS: Record<string, { bg: string; legend: string }> = {
  completed: { bg: "bg-green-500", legend: "bg-green-500" },
  failed: { bg: "bg-red-500", legend: "bg-red-500" },
  timeout: { bg: "bg-orange-500", legend: "bg-orange-500" },
  running: { bg: "bg-blue-500", legend: "bg-blue-500" },
  requested: { bg: "bg-yellow-400", legend: "bg-yellow-400" },
  scheduled: { bg: "bg-yellow-500", legend: "bg-yellow-500" },
  scheduling: { bg: "bg-yellow-300", legend: "bg-yellow-300" },
  cancelled: { bg: "bg-gray-400", legend: "bg-gray-400" },
  canceling: { bg: "bg-gray-300", legend: "bg-gray-300" },
  abandoned: { bg: "bg-purple-400", legend: "bg-purple-400" },
  online: { bg: "bg-green-500", legend: "bg-green-500" },
  offline: { bg: "bg-red-400", legend: "bg-red-400" },
  draining: { bg: "bg-yellow-500", legend: "bg-yellow-500" },
};

function getStatusColor(status: string): string {
  return STATUS_COLORS[status]?.bg || "bg-gray-400";
}

interface StackedBarChartProps {
  points: TimeSeriesPoint[];
  rangeHours: number;
  height?: number;
}

function StackedBarChart({
  points,
  rangeHours,
  height = 120,
}: StackedBarChartProps) {
  const [hoveredIdx, setHoveredIdx] = useState<number | null>(null);

  const { buckets, allLabels, maxTotal } = useMemo(
    () => buildStackedBucketModel(points),
    [points],
  );

  if (buckets.length === 0) {
    return (
      <div
        className="flex items-center justify-center text-gray-400 text-xs"
        style={{ height }}
      >
        No data in this time range
      </div>
    );
  }

  const labelEvery =
    buckets.length > 24
      ? Math.ceil(buckets.length / 8)
      : buckets.length > 12
        ? 2
        : 1;

  return (
    <div>
      {/* Legend */}
      <div className="flex flex-wrap gap-x-3 gap-y-1 mb-2">
        {allLabels.map((label) => (
          <div
            key={label}
            className="flex items-center gap-1 text-[10px] text-gray-600"
          >
            <div
              className={`w-2 h-2 rounded-sm ${STATUS_COLORS[label]?.legend || "bg-gray-400"}`}
            />
            {label}
          </div>
        ))}
      </div>

      <div className="relative" style={{ height: height + 24 }}>
        {/* Tooltip */}
        {hoveredIdx !== null && buckets[hoveredIdx] && (
          <div className="absolute -top-1 left-1/2 -translate-x-1/2 z-10 bg-gray-800 text-white text-xs rounded px-2 py-1 whitespace-nowrap pointer-events-none shadow-lg">
            <div className="font-medium mb-0.5">
              {formatBucketTooltip(buckets[hoveredIdx].bucket)}
            </div>
            {Array.from(buckets[hoveredIdx].byLabel.entries()).map(
              ([label, count]) => (
                <div key={label}>
                  {label}: {count}
                </div>
              ),
            )}
          </div>
        )}

        {/* Bars */}
        <div className="flex items-end gap-px w-full" style={{ height }}>
          {buckets.map((b, i) => {
            const totalPct = (b.total / maxTotal) * 100;
            return (
              <div
                key={b.bucket}
                className="flex-1 min-w-0 relative"
                style={{ height: "100%" }}
                onMouseEnter={() => setHoveredIdx(i)}
                onMouseLeave={() => setHoveredIdx(null)}
              >
                <div
                  className="absolute bottom-0 inset-x-0 flex flex-col-reverse"
                  style={{
                    height: `${Math.max(totalPct, b.total > 0 ? 2 : 0)}%`,
                    minHeight: b.total > 0 ? "2px" : "0",
                  }}
                >
                  {allLabels.map((label) => {
                    const count = b.byLabel.get(label) || 0;
                    if (count === 0) return null;
                    const segmentPct = (count / b.total) * 100;
                    return (
                      <div
                        key={label}
                        className={`w-full ${getStatusColor(label)} ${
                          hoveredIdx === i ? "opacity-100" : "opacity-80"
                        } transition-opacity`}
                        style={{
                          height: `${segmentPct}%`,
                          minHeight: "1px",
                        }}
                      />
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>

        {/* X-axis labels */}
        <div className="flex items-start mt-1 h-5">
          {buckets.map((b, i) =>
            i % labelEvery === 0 ? (
              <div
                key={b.bucket}
                className="flex-1 text-center text-[9px] text-gray-400 truncate"
                style={{ minWidth: 0 }}
              >
                {formatBucketLabel(b.bucket, rangeHours)}
              </div>
            ) : (
              <div key={b.bucket} className="flex-1" />
            ),
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// FailureRateCard
// ---------------------------------------------------------------------------

interface FailureRateCardProps {
  summary: FailureRateSummary;
}

function FailureRateCard({ summary }: FailureRateCardProps) {
  const rate = summary.failure_rate_pct;
  const rateColor =
    rate === 0
      ? "text-green-600"
      : rate < 5
        ? "text-yellow-600"
        : rate < 20
          ? "text-orange-600"
          : "text-red-600";

  const ringColor =
    rate === 0
      ? "stroke-green-500"
      : rate < 5
        ? "stroke-yellow-500"
        : rate < 20
          ? "stroke-orange-500"
          : "stroke-red-500";

  // SVG ring gauge
  const radius = 40;
  const circumference = 2 * Math.PI * radius;
  const failureArc = (rate / 100) * circumference;
  const successArc = circumference - failureArc;

  return (
    <div className="flex items-center gap-6">
      {/* Ring gauge */}
      <div className="relative flex-shrink-0">
        <svg width="100" height="100" className="-rotate-90">
          {/* Background ring */}
          <circle
            cx="50"
            cy="50"
            r={radius}
            fill="none"
            strokeWidth="8"
            className="stroke-gray-200"
          />
          {/* Success arc */}
          <circle
            cx="50"
            cy="50"
            r={radius}
            fill="none"
            strokeWidth="8"
            className="stroke-green-400"
            strokeDasharray={`${successArc} ${circumference}`}
            strokeLinecap="round"
          />
          {/* Failure arc */}
          {rate > 0 && (
            <circle
              cx="50"
              cy="50"
              r={radius}
              fill="none"
              strokeWidth="8"
              className={ringColor}
              strokeDasharray={`${failureArc} ${circumference}`}
              strokeDashoffset={`${-successArc}`}
              strokeLinecap="round"
            />
          )}
        </svg>
        <div className="absolute inset-0 flex items-center justify-center">
          <span className={`text-lg font-bold ${rateColor}`}>
            {rate.toFixed(1)}%
          </span>
        </div>
      </div>

      {/* Breakdown */}
      <div className="space-y-1.5 text-sm">
        <div className="flex items-center gap-2">
          <CheckCircle className="h-4 w-4 text-green-500" />
          <span className="text-gray-600">Completed:</span>
          <span className="font-medium text-gray-900">
            {summary.completed_count}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <AlertTriangle className="h-4 w-4 text-red-500" />
          <span className="text-gray-600">Failed:</span>
          <span className="font-medium text-gray-900">
            {summary.failed_count}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <AlertTriangle className="h-4 w-4 text-orange-500" />
          <span className="text-gray-600">Timeout:</span>
          <span className="font-medium text-gray-900">
            {summary.timeout_count}
          </span>
        </div>
        <div className="text-xs text-gray-400 mt-1">
          {summary.total_terminal} total terminal executions
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// StatCard — simple metric card with icon and value
// ---------------------------------------------------------------------------

interface StatCardProps {
  icon: React.ReactNode;
  label: string;
  value: number | string;
  subtext?: string;
  color?: string;
}

function StatCard({
  icon,
  label,
  value,
  subtext,
  color = "text-blue-600",
}: StatCardProps) {
  return (
    <div className="flex items-center gap-3">
      <div className={`${color} opacity-70`}>{icon}</div>
      <div>
        <p className="text-xs text-gray-500">{label}</p>
        <p className={`text-2xl font-bold ${color}`}>{value}</p>
        {subtext && <p className="text-[10px] text-gray-400">{subtext}</p>}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// AnalyticsDashboard — main composite widget
// ---------------------------------------------------------------------------

interface AnalyticsDashboardProps {
  /** The analytics data (from useDashboardAnalytics hook) */
  data: DashboardAnalytics | undefined;
  /** Whether the data is loading */
  isLoading: boolean;
  /** Error object if the fetch failed */
  error: Error | null;
  /** Current time range in hours */
  hours: TimeRangeHours;
  /** Callback to change the time range */
  onHoursChange: (h: TimeRangeHours) => void;
}

export default function AnalyticsDashboard({
  data,
  isLoading,
  error,
  hours,
  onHoursChange,
}: AnalyticsDashboardProps) {
  // Extract sub-properties so useMemo deps match what the React Compiler infers
  const executionThroughput = data?.execution_throughput;
  const eventVolume = data?.event_volume;
  const enforcementVolume = data?.enforcement_volume;

  const executionBuckets = useMemo(() => {
    if (!executionThroughput) return [];
    return toSortedBucketTotals(executionThroughput);
  }, [executionThroughput]);

  const eventBuckets = useMemo(() => {
    if (!eventVolume) return [];
    return toSortedBucketTotals(eventVolume);
  }, [eventVolume]);

  const enforcementBuckets = useMemo(() => {
    if (!enforcementVolume) return [];
    return toSortedBucketTotals(enforcementVolume);
  }, [enforcementVolume]);

  const totalExecutions = useMemo(
    () => executionBuckets.reduce((s, b) => s + b.value, 0),
    [executionBuckets],
  );

  const totalEvents = useMemo(
    () => eventBuckets.reduce((s, b) => s + b.value, 0),
    [eventBuckets],
  );

  const totalEnforcements = useMemo(
    () => enforcementBuckets.reduce((s, b) => s + b.value, 0),
    [enforcementBuckets],
  );

  // Loading state
  if (isLoading && !data) {
    return (
      <div className="bg-white rounded-lg shadow p-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <BarChart3 className="h-5 w-5 text-gray-500" />
            <h2 className="text-lg font-semibold text-gray-900">Analytics</h2>
          </div>
          <TimeRangeSelector value={hours} onChange={onHoursChange} />
        </div>
        <div className="flex items-center justify-center py-12">
          <div className="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  // Error state
  if (error) {
    return (
      <div className="bg-white rounded-lg shadow p-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <BarChart3 className="h-5 w-5 text-gray-500" />
            <h2 className="text-lg font-semibold text-gray-900">Analytics</h2>
          </div>
          <TimeRangeSelector value={hours} onChange={onHoursChange} />
        </div>
        <div className="bg-red-50 border border-red-200 text-red-700 rounded p-3 text-sm">
          Failed to load analytics data.{" "}
          {error.message && (
            <span className="text-red-500">{error.message}</span>
          )}
        </div>
      </div>
    );
  }

  if (!data) return null;

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <BarChart3 className="h-5 w-5 text-gray-500" />
          <h2 className="text-lg font-semibold text-gray-900">Analytics</h2>
          {isLoading && (
            <div className="inline-block animate-spin rounded-full h-4 w-4 border-b-2 border-blue-400" />
          )}
        </div>
        <TimeRangeSelector value={hours} onChange={onHoursChange} />
      </div>

      {/* Summary stat cards */}
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
        <div className="bg-white rounded-lg shadow p-4">
          <StatCard
            icon={<Activity className="h-5 w-5" />}
            label={`Executions (${hours}h)`}
            value={totalExecutions}
            color="text-blue-600"
          />
        </div>
        <div className="bg-white rounded-lg shadow p-4">
          <StatCard
            icon={<Zap className="h-5 w-5" />}
            label={`Events (${hours}h)`}
            value={totalEvents}
            color="text-indigo-600"
          />
        </div>
        <div className="bg-white rounded-lg shadow p-4">
          <StatCard
            icon={<CheckCircle className="h-5 w-5" />}
            label={`Enforcements (${hours}h)`}
            value={totalEnforcements}
            color="text-purple-600"
          />
        </div>
      </div>

      {/* Charts row 1: throughput + failure rate */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Execution throughput */}
        <div className="bg-white rounded-lg shadow p-6 lg:col-span-2">
          <h3 className="text-sm font-semibold text-gray-700 mb-3 flex items-center gap-1.5">
            <Activity className="h-4 w-4 text-blue-500" />
            Execution Throughput
          </h3>
          <MiniBarChart
            buckets={executionBuckets}
            rangeHours={hours}
            barColor="bg-blue-500"
            height={140}
          />
        </div>

        {/* Failure rate */}
        <div className="bg-white rounded-lg shadow p-6">
          <h3 className="text-sm font-semibold text-gray-700 mb-3 flex items-center gap-1.5">
            <AlertTriangle className="h-4 w-4 text-red-500" />
            Failure Rate
          </h3>
          <FailureRateCard summary={data.failure_rate} />
        </div>
      </div>

      {/* Charts row 2: status breakdown + event volume */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Execution status breakdown */}
        <div className="bg-white rounded-lg shadow p-6">
          <h3 className="text-sm font-semibold text-gray-700 mb-3 flex items-center gap-1.5">
            <BarChart3 className="h-4 w-4 text-green-500" />
            Execution Status Over Time
          </h3>
          <StackedBarChart
            points={data.execution_status}
            rangeHours={hours}
            height={140}
          />
        </div>

        {/* Event volume */}
        <div className="bg-white rounded-lg shadow p-6">
          <h3 className="text-sm font-semibold text-gray-700 mb-3 flex items-center gap-1.5">
            <Zap className="h-4 w-4 text-indigo-500" />
            Event Volume
          </h3>
          <MiniBarChart
            buckets={eventBuckets}
            rangeHours={hours}
            barColor="bg-indigo-500"
            height={140}
          />
        </div>
      </div>

      {/* Charts row 3: enforcements + worker health */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Enforcement volume */}
        <div className="bg-white rounded-lg shadow p-6">
          <h3 className="text-sm font-semibold text-gray-700 mb-3 flex items-center gap-1.5">
            <CheckCircle className="h-4 w-4 text-purple-500" />
            Enforcement Volume
          </h3>
          <MiniBarChart
            buckets={enforcementBuckets}
            rangeHours={hours}
            barColor="bg-purple-500"
            height={120}
          />
        </div>

        {/* Worker status */}
        <div className="bg-white rounded-lg shadow p-6">
          <h3 className="text-sm font-semibold text-gray-700 mb-3 flex items-center gap-1.5">
            <Server className="h-4 w-4 text-teal-500" />
            Worker Status Transitions
          </h3>
          <StackedBarChart
            points={data.worker_status}
            rangeHours={hours}
            height={120}
          />
        </div>
      </div>
    </div>
  );
}

// Re-export sub-components and types for standalone use
export {
  MiniBarChart,
  StackedBarChart,
  FailureRateCard,
  StatCard,
  TimeRangeSelector,
};
export type { TimeRangeHours };
