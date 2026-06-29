import { useState } from "react";
import { formatDistanceToNow } from "date-fns";
import {
  ChevronDown,
  ChevronRight,
  History,
  Filter,
  ChevronLeft,
  ChevronsLeft,
  ChevronsRight,
} from "lucide-react";
import {
  useEntityHistory,
  type HistoryEntityType,
  type HistoryRecord,
  type HistoryQueryParams,
} from "@/hooks/useHistory";

interface EntityHistoryPanelProps {
  /** The type of entity whose history to display */
  entityType: HistoryEntityType;
  /** The entity's primary key */
  entityId: number;
  /** Optional title override (default: "Change History") */
  title?: string;
  /** Whether the panel starts collapsed (default: true) */
  defaultCollapsed?: boolean;
  /** Number of items per page (default: 10) */
  pageSize?: number;
}

/**
 * A reusable panel that displays the change history for an entity.
 *
 * Queries the TimescaleDB history hypertables via the API and renders
 * a timeline of changes with expandable details showing old/new values.
 */
export default function EntityHistoryPanel({
  entityType,
  entityId,
  title = "Change History",
  defaultCollapsed = true,
  pageSize = 10,
}: EntityHistoryPanelProps) {
  const [isCollapsed, setIsCollapsed] = useState(defaultCollapsed);
  const [page, setPage] = useState(1);
  const [operationFilter, setOperationFilter] = useState<string>("");
  const [fieldFilter, setFieldFilter] = useState<string>("");
  const [showFilters, setShowFilters] = useState(false);

  const params: HistoryQueryParams = {
    page,
    page_size: pageSize,
    ...(operationFilter ? { operation: operationFilter } : {}),
    ...(fieldFilter ? { changed_field: fieldFilter } : {}),
  };

  const { data, isLoading, error } = useEntityHistory(
    entityType,
    entityId,
    params,
    !isCollapsed && !!entityId,
  );

  const records = data?.items ?? [];
  const pagination = data?.pagination;
  const totalPages = pagination?.total_pages ?? 1;
  const totalItems = pagination?.total_items ?? 0;

  const handleClearFilters = () => {
    setOperationFilter("");
    setFieldFilter("");
    setPage(1);
  };

  const hasActiveFilters = !!operationFilter || !!fieldFilter;

  return (
    <div className="bg-white rounded-lg shadow">
      {/* Header — always visible */}
      <button
        onClick={() => setIsCollapsed(!isCollapsed)}
        className="w-full px-6 py-4 flex items-center justify-between border-b border-gray-200 hover:bg-gray-50 transition-colors"
      >
        <div className="flex items-center gap-2">
          <History className="h-5 w-5 text-gray-500" />
          <h2 className="text-lg font-semibold text-gray-900">{title}</h2>
          {totalItems > 0 && !isCollapsed && (
            <span className="ml-2 px-2 py-0.5 text-xs font-medium bg-gray-100 text-gray-600 rounded-full">
              {totalItems}
            </span>
          )}
        </div>
        {isCollapsed ? (
          <ChevronRight className="h-5 w-5 text-gray-400" />
        ) : (
          <ChevronDown className="h-5 w-5 text-gray-400" />
        )}
      </button>

      {/* Body — only when expanded */}
      {!isCollapsed && (
        <div className="px-6 py-4">
          {/* Filter bar */}
          <div className="mb-4">
            <button
              onClick={() => setShowFilters(!showFilters)}
              className="flex items-center gap-1 text-sm text-gray-500 hover:text-gray-700"
            >
              <Filter className="h-3.5 w-3.5" />
              <span>Filters</span>
              {hasActiveFilters && (
                <span className="ml-1 h-2 w-2 rounded-full bg-blue-500" />
              )}
            </button>

            {showFilters && (
              <div className="mt-2 flex flex-wrap gap-3 items-end">
                <div>
                  <label className="block text-xs text-gray-500 mb-1">
                    Operation
                  </label>
                  <select
                    value={operationFilter}
                    onChange={(e) => {
                      setOperationFilter(e.target.value);
                      setPage(1);
                    }}
                    className="text-sm border border-gray-300 rounded px-2 py-1.5 bg-white"
                  >
                    <option value="">All</option>
                    <option value="INSERT">INSERT</option>
                    <option value="UPDATE">UPDATE</option>
                    <option value="DELETE">DELETE</option>
                  </select>
                </div>
                <div>
                  <label className="block text-xs text-gray-500 mb-1">
                    Changed Field
                  </label>
                  <input
                    type="text"
                    value={fieldFilter}
                    onChange={(e) => {
                      setFieldFilter(e.target.value);
                      setPage(1);
                    }}
                    placeholder="e.g. status"
                    className="text-sm border border-gray-300 rounded px-2 py-1.5 w-36"
                  />
                </div>
                {hasActiveFilters && (
                  <button
                    onClick={handleClearFilters}
                    className="text-xs text-blue-600 hover:text-blue-800 pb-1"
                  >
                    Clear filters
                  </button>
                )}
              </div>
            )}
          </div>

          {/* Loading state */}
          {isLoading && (
            <div className="flex items-center justify-center py-8">
              <div className="inline-block animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600" />
            </div>
          )}

          {/* Error state */}
          {error && (
            <div className="bg-red-50 border border-red-200 text-red-700 rounded p-3 text-sm">
              Failed to load history:{" "}
              {error instanceof Error ? error.message : "Unknown error"}
            </div>
          )}

          {/* Empty state */}
          {!isLoading && !error && records.length === 0 && (
            <p className="text-sm text-gray-500 py-4 text-center">
              {hasActiveFilters
                ? "No history records match the current filters."
                : "No change history recorded yet."}
            </p>
          )}

          {/* Records list */}
          {!isLoading && !error && records.length > 0 && (
            <div className="space-y-1">
              {records.map((record, idx) => (
                <HistoryRecordRow
                  key={`${record.time}-${idx}`}
                  record={record}
                />
              ))}
            </div>
          )}

          {/* Pagination */}
          {!isLoading && totalPages > 1 && (
            <div className="mt-4 flex items-center justify-between text-sm">
              <span className="text-gray-500">
                Page {page} of {totalPages} ({totalItems} records)
              </span>
              <div className="flex items-center gap-1">
                <PaginationButton
                  onClick={() => setPage(1)}
                  disabled={page <= 1}
                  title="First page"
                >
                  <ChevronsLeft className="h-4 w-4" />
                </PaginationButton>
                <PaginationButton
                  onClick={() => setPage(page - 1)}
                  disabled={page <= 1}
                  title="Previous page"
                >
                  <ChevronLeft className="h-4 w-4" />
                </PaginationButton>
                <PaginationButton
                  onClick={() => setPage(page + 1)}
                  disabled={page >= totalPages}
                  title="Next page"
                >
                  <ChevronRight className="h-4 w-4" />
                </PaginationButton>
                <PaginationButton
                  onClick={() => setPage(totalPages)}
                  disabled={page >= totalPages}
                  title="Last page"
                >
                  <ChevronsRight className="h-4 w-4" />
                </PaginationButton>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function PaginationButton({
  onClick,
  disabled,
  title,
  children,
}: {
  onClick: () => void;
  disabled: boolean;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={title}
      className="p-1 rounded hover:bg-gray-100 disabled:opacity-30 disabled:cursor-not-allowed"
    >
      {children}
    </button>
  );
}

/**
 * A single history record displayed as a collapsible row.
 */
function HistoryRecordRow({ record }: { record: HistoryRecord }) {
  const [expanded, setExpanded] = useState(false);

  const time = new Date(record.time);
  const relativeTime = formatDistanceToNow(time, { addSuffix: true });

  return (
    <div className="border border-gray-100 rounded">
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center gap-3 px-3 py-2 text-left hover:bg-gray-50 transition-colors text-sm"
      >
        {/* Expand/collapse indicator */}
        {expanded ? (
          <ChevronDown className="h-3.5 w-3.5 text-gray-400 flex-shrink-0" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 text-gray-400 flex-shrink-0" />
        )}

        {/* Operation badge */}
        <OperationBadge operation={record.operation} />

        {/* Changed fields summary */}
        <span className="text-gray-700 truncate flex-1">
          {record.operation === "INSERT" && "Entity created"}
          {record.operation === "DELETE" && "Entity deleted"}
          {record.operation === "UPDATE" &&
            record.changed_fields.length > 0 && (
              <>
                Changed{" "}
                <span className="font-medium">
                  {record.changed_fields.join(", ")}
                </span>
              </>
            )}
          {record.operation === "UPDATE" &&
            record.changed_fields.length === 0 &&
            "Updated"}
        </span>

        {/* Timestamp */}
        <span
          className="text-xs text-gray-400 flex-shrink-0"
          title={time.toISOString()}
        >
          {relativeTime}
        </span>
      </button>

      {/* Expanded detail */}
      {expanded && (
        <div className="px-3 pb-3 pt-1 border-t border-gray-100">
          {/* Timestamp detail */}
          <p className="text-xs text-gray-400 mb-2">
            {time.toLocaleString()} (UTC: {time.toISOString()})
          </p>

          {/* Field-level diffs */}
          {record.operation === "UPDATE" &&
            record.changed_fields.length > 0 && (
              <div className="space-y-2">
                {record.changed_fields.map((field) => (
                  <FieldDiff
                    key={field}
                    field={field}
                    oldValue={record.old_values?.[field]}
                    newValue={record.new_values?.[field]}
                  />
                ))}
              </div>
            )}

          {/* INSERT — show new_values */}
          {record.operation === "INSERT" && record.new_values && (
            <div>
              <p className="text-xs font-medium text-gray-500 mb-1">
                Initial values
              </p>
              <JsonBlock value={record.new_values} />
            </div>
          )}

          {/* DELETE — show old_values if available */}
          {record.operation === "DELETE" && record.old_values && (
            <div>
              <p className="text-xs font-medium text-gray-500 mb-1">
                Values at deletion
              </p>
              <JsonBlock value={record.old_values} />
            </div>
          )}

          {/* Fallback when there's nothing to show */}
          {!record.old_values && !record.new_values && (
            <p className="text-xs text-gray-400 italic">
              No field-level details recorded.
            </p>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Colored badge for the operation type.
 */
function OperationBadge({ operation }: { operation: string }) {
  const colors: Record<string, string> = {
    INSERT: "bg-green-100 text-green-700",
    UPDATE: "bg-blue-100 text-blue-700",
    DELETE: "bg-red-100 text-red-700",
  };

  return (
    <span
      className={`px-1.5 py-0.5 text-[10px] font-semibold rounded flex-shrink-0 ${colors[operation] ?? "bg-gray-100 text-gray-700"}`}
    >
      {operation}
    </span>
  );
}

/**
 * Renders a single field's old → new diff.
 */
function FieldDiff({
  field,
  oldValue,
  newValue,
}: {
  field: string;
  oldValue: unknown;
  newValue: unknown;
}) {
  const isSimple = typeof oldValue !== "object" && typeof newValue !== "object";

  return (
    <div className="text-xs">
      <p className="font-medium text-gray-600 mb-0.5">{field}</p>
      {isSimple ? (
        <div className="flex items-center gap-2 flex-wrap">
          <span className="bg-red-50 text-red-700 px-1.5 py-0.5 rounded line-through">
            {formatValue(oldValue)}
          </span>
          <span className="text-gray-400">→</span>
          <span className="bg-green-50 text-green-700 px-1.5 py-0.5 rounded">
            {formatValue(newValue)}
          </span>
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-2">
          <div>
            <p className="text-[10px] text-gray-400 mb-0.5">Before</p>
            <JsonBlock value={oldValue} />
          </div>
          <div>
            <p className="text-[10px] text-gray-400 mb-0.5">After</p>
            <JsonBlock value={newValue} />
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Format a scalar value for display.
 */
function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "null";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

/**
 * Renders a JSONB value in a code block.
 */
function JsonBlock({ value }: { value: unknown }) {
  if (value === null || value === undefined) {
    return <span className="text-gray-400 text-xs italic">null</span>;
  }

  const formatted =
    typeof value === "object" ? JSON.stringify(value, null, 2) : String(value);

  return (
    <pre className="bg-gray-50 rounded p-2 text-[11px] text-gray-700 overflow-x-auto max-h-48 whitespace-pre-wrap break-all">
      {formatted}
    </pre>
  );
}
