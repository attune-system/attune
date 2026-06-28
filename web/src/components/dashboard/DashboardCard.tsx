import type {
  DashboardCardSpec,
  DashboardSourceMeta,
  DashboardSourceResult,
  DashboardSourceStatus,
} from "@/types/dashboard";
import { DashboardChartRenderer } from "@/components/dashboard/charts/DashboardChartRenderer";
import {
  asNumber,
  formatValue,
  getLevelColor,
  pickPreferredColumns,
  toRows,
} from "@/components/dashboard/charts/foundation";

function statusBadgeClass(status: DashboardSourceStatus): string {
  switch (status) {
    case "ok":
      return "bg-green-100 text-green-700";
    case "empty":
      return "bg-gray-100 text-gray-700";
    case "stale":
      return "bg-amber-100 text-amber-800";
    case "partial":
      return "bg-yellow-100 text-yellow-800";
    case "forbidden":
      return "bg-purple-100 text-purple-800";
    case "invalid":
      return "bg-orange-100 text-orange-800";
    case "error":
      return "bg-red-100 text-red-800";
    default:
      return "bg-gray-100 text-gray-700";
  }
}

function statusLabel(status: DashboardSourceStatus): string {
  if (status === "ok") return "OK";
  if (status === "empty") return "Empty";
  if (status === "stale") return "Stale";
  if (status === "partial") return "Partial";
  if (status === "forbidden") return "Forbidden";
  if (status === "invalid") return "Invalid";
  return "Error";
}

function EmptyState({ message }: { message: string }) {
  return (
    <div className="h-full min-h-32 flex items-center justify-center text-sm text-gray-500">
      {message}
    </div>
  );
}

function ErrorState({
  source,
  onRetry,
}: {
  source: DashboardSourceResult;
  onRetry?: () => void;
}) {
  return (
    <div className="h-full min-h-32 flex flex-col items-center justify-center gap-3 text-center px-3">
      <p className="text-sm text-gray-700">
        {source.error?.message || "Failed to load source data."}
      </p>
      {source.error?.code && (
        <p className="text-xs text-gray-500">Code: {source.error.code}</p>
      )}
      {source.error?.retryable && onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="text-xs px-2 py-1 rounded bg-blue-600 text-white hover:bg-blue-700"
        >
          Retry
        </button>
      )}
    </div>
  );
}

function ForbiddenState({ source }: { source: DashboardSourceResult }) {
  return (
    <div className="h-full min-h-32 flex items-center justify-center text-sm text-purple-800 px-3 text-center">
      {source.error?.message || "You are not authorized to view this source."}
    </div>
  );
}

function StatRenderer({
  card,
  source,
}: {
  card: DashboardCardSpec;
  source: DashboardSourceResult;
}) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyState message="No values in selected range." />;

  const valueField = card.visualization.value_field || source.meta.ordering[0] || "value";
  const row = rows[0];
  const value = row?.[valueField];
  const formatted = formatValue(
    value,
    card.visualization.format,
    source.meta.unit_hints[valueField],
  );

  return (
    <div className="h-full flex flex-col justify-center items-center">
      <p className="text-3xl font-semibold text-gray-900">{formatted}</p>
      <p className="text-xs text-gray-500 mt-1">{valueField}</p>
    </div>
  );
}

function KpiRenderer({
  card,
  source,
}: {
  card: DashboardCardSpec;
  source: DashboardSourceResult;
}) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyState message="No values in selected range." />;

  const valueField = card.visualization.value_field || source.meta.ordering[0] || "value";
  const row = rows[0];
  const rawValue = row?.[valueField];
  const formatted = formatValue(
    rawValue,
    card.visualization.format,
    source.meta.unit_hints[valueField],
  );
  const numericValue = asNumber(rawValue);

  let level: "good" | "warning" | "bad" | undefined;
  if (numericValue !== null) {
    const bands = card.visualization.bands;
    if (bands && bands.length > 0) {
      const matchingBand = bands.find((band, index) => {
        if (index === bands.length - 1) {
          return numericValue >= band.from && numericValue <= band.to;
        }
        return numericValue >= band.from && numericValue < band.to;
      });
      if (matchingBand && ["good", "warning", "bad"].includes(matchingBand.level)) {
        level = matchingBand.level as "good" | "warning" | "bad";
      }
    } else if (card.visualization.min !== undefined || card.visualization.max !== undefined) {
      const configuredMin = card.visualization.min ?? 0;
      const configuredMax = card.visualization.max ?? 100;
      const minValue = Math.min(configuredMin, configuredMax);
      const maxValue = Math.max(minValue + 1, Math.max(configuredMin, configuredMax));
      const span = Math.max(1, maxValue - minValue);
      const warningStart = minValue + span * 0.6;
      const badStart = minValue + span * 0.85;
      const mode = card.visualization.mode === "low_is_bad" ? "low_is_bad" : "high_is_bad";
      if (mode === "low_is_bad") {
        if (numericValue >= badStart) level = "good";
        else if (numericValue >= warningStart) level = "warning";
        else level = "bad";
      } else {
        if (numericValue >= badStart) level = "bad";
        else if (numericValue >= warningStart) level = "warning";
        else level = "good";
      }
    }
  }

  const levelLabel = level ? level.toUpperCase() : "UNSPECIFIED";
  const badgeColor = getLevelColor(level, "#6b7280");
  const badgeStyle = {
    backgroundColor: `${badgeColor}20`,
    borderColor: `${badgeColor}66`,
    color: badgeColor,
  };

  return (
    <div className="h-full flex flex-col justify-center items-center">
      <p className="text-3xl font-semibold text-gray-900">{formatted}</p>
      <span
        className="mt-2 inline-flex items-center rounded-full border px-2 py-0.5 text-[11px] font-semibold tracking-wide"
        style={badgeStyle}
      >
        {levelLabel}
      </span>
      <p className="text-xs text-gray-500 mt-1">{valueField}</p>
    </div>
  );
}

function TableRenderer({
  card,
  source,
}: {
  card: DashboardCardSpec;
  source: DashboardSourceResult;
}) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyState message="No rows in selected range." />;

  const columns = pickPreferredColumns(rows, source.meta.ordering);

  return (
    <div className="h-full overflow-auto">
      <table className="min-w-full text-xs">
        <thead className="sticky top-0 bg-gray-50">
          <tr>
            {columns.map((column) => (
              <th
                key={column}
                className="text-left px-2 py-1 font-medium text-gray-600"
              >
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, rowIndex) => (
            <tr
              key={`${card.id}-row-${rowIndex}`}
              className={rowIndex % 2 === 0 ? "bg-white" : "bg-gray-50/50"}
            >
              {columns.map((column) => (
                <td
                  key={`${rowIndex}-${column}`}
                  className="px-2 py-1 text-gray-800 whitespace-nowrap"
                >
                  {formatValue(
                    row[column],
                    card.visualization.format,
                    source.meta.unit_hints[column],
                  )}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function renderByType(card: DashboardCardSpec, source: DashboardSourceResult) {
  const type = card.visualization.type;
  if (type === "stat") {
    return <StatRenderer card={card} source={source} />;
  }
  if (type === "kpi") return <KpiRenderer card={card} source={source} />;
  if (type === "table") {
    return <TableRenderer card={card} source={source} />;
  }

  return <DashboardChartRenderer card={card} source={source} />;
}

function MetaInfo({ meta }: { meta: DashboardSourceMeta }) {
  const hasOrdering = meta.ordering.length > 0;
  const hasBucket = Boolean(meta.bucket_size);
  const hasTruncated = meta.truncated;
  if (!hasOrdering && !hasBucket && !hasTruncated) {
    return null;
  }

  return (
    <div className="relative group">
      <span className="inline-flex h-5 w-5 items-center justify-center rounded-full border border-gray-300 text-[11px] font-semibold text-gray-600 cursor-default">
        i
      </span>
      <div className="pointer-events-none absolute right-0 top-6 z-20 hidden min-w-56 max-w-80 rounded border border-gray-200 bg-white px-3 py-2 text-[11px] text-gray-600 shadow-lg group-hover:block">
        <div><span className="font-medium text-gray-700">freshness:</span> {meta.freshness_mode}</div>
        {meta.bucket_size && (
          <div><span className="font-medium text-gray-700">bucket:</span> {meta.bucket_size}</div>
        )}
        {meta.truncated && <div className="text-amber-700 font-medium">truncated</div>}
        {meta.ordering.length > 0 && (
          <div className="break-words">
            <span className="font-medium text-gray-700">ordering:</span> {meta.ordering.join(", ")}
          </div>
        )}
      </div>
    </div>
  );
}

interface DashboardCardProps {
  card: DashboardCardSpec;
  source?: DashboardSourceResult;
  isRefreshing?: boolean;
  onRetry?: () => void;
}

export function DashboardCard({
  card,
  source,
  isRefreshing,
  onRetry,
}: DashboardCardProps) {
  const inferredSource: DashboardSourceResult =
    source ||
    ({
      source_id: card.source,
      source_type: "unknown",
      status: "error",
      data: null,
      meta: {
        authorization_mode: "operator_global",
        freshness_mode: "raw_only",
        aggregate_watermark: null,
        cache_hit: false,
        bucket_size: null,
        truncated: false,
        unit_hints: {},
        ordering: [],
        authorized_refs: null,
      },
      error: {
        code: "missing_source",
        message: "Source result missing from response.",
        retryable: true,
        details: null,
      },
    } satisfies DashboardSourceResult);

  const status = inferredSource.status;
  const renderContent = () => {
    if (status === "forbidden") {
      return <ForbiddenState source={inferredSource} />;
    }
    if (status === "invalid" || status === "error") {
      return <ErrorState source={inferredSource} onRetry={onRetry} />;
    }
    if (status === "empty") {
      return <EmptyState message="No data available for current filters." />;
    }
    return renderByType(card, inferredSource);
  };

  return (
    <article className="bg-white border border-gray-200 rounded-lg shadow-sm p-4 h-full flex flex-col">
      <header className="flex items-start justify-between gap-2 mb-3">
        <div>
          <h3 className="text-sm font-semibold text-gray-900">{card.title}</h3>
          {card.subtitle && (
            <p className="text-xs text-gray-500 mt-0.5">{card.subtitle}</p>
          )}
        </div>
        <div className="flex items-center gap-2">
          <MetaInfo meta={inferredSource.meta} />
          {isRefreshing && (
            <span className="text-[10px] text-blue-600">refreshing…</span>
          )}
          <span
            className={`text-[10px] px-2 py-0.5 rounded-full ${statusBadgeClass(status)}`}
          >
            {statusLabel(status)}
          </span>
        </div>
      </header>
      <div className="flex-1 min-h-0">{renderContent()}</div>
    </article>
  );
}
