import type {
  DashboardCardSpec,
  DashboardSourceMeta,
  DashboardSourceResult,
  DashboardSourceStatus,
} from "@/types/dashboard";
import { DashboardChartRenderer } from "@/components/dashboard/charts/DashboardChartRenderer";
import {
  formatValue,
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
  if (type === "stat" || type === "kpi") {
    return <StatRenderer card={card} source={source} />;
  }
  if (type === "table") {
    return <TableRenderer card={card} source={source} />;
  }

  return <DashboardChartRenderer card={card} source={source} />;
}

function MetaInfo({ meta }: { meta: DashboardSourceMeta }) {
  return (
    <div className="mt-3 pt-2 border-t border-gray-100 text-[10px] text-gray-500 flex items-center gap-2 flex-wrap">
      <span>freshness: {meta.freshness_mode}</span>
      {meta.bucket_size && <span>bucket: {meta.bucket_size}</span>}
      {meta.truncated && <span className="text-amber-700">truncated</span>}
      {meta.ordering.length > 0 && <span>ordering: {meta.ordering.join(", ")}</span>}
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
      <MetaInfo meta={inferredSource.meta} />
    </article>
  );
}
