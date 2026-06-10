import { Link, useSearchParams } from "react-router-dom";
import { useEnforcements } from "@/hooks/useEvents";
import { useEnforcementStream } from "@/hooks/useEnforcementStream";
import { EnforcementStatus } from "@/api";
import type { EnforcementSummary } from "@/api";
import { useState, useMemo, memo, useCallback, useEffect } from "react";
import { Search, X } from "lucide-react";
import MultiSelect from "@/components/common/MultiSelect";
import AutocompleteInput from "@/components/common/AutocompleteInput";
import {
  useFilterSuggestions,
  useMergedSuggestions,
} from "@/hooks/useFilterSuggestions";
import Pagination from "@/components/executions/Pagination";
import LiveStreamControl, {
  DEFAULT_LIVE_LIST_MAX_ITEMS,
} from "@/components/common/LiveStreamControl";
import PackIcon from "@/components/common/PackIcon";
import { packRefFromComponentRef } from "@/utils/packIcons";

// Memoized filter input component for non-ref fields (e.g. Event ID)
const FilterInput = memo(
  ({
    label,
    value,
    onChange,
    placeholder,
  }: {
    label: string;
    value: string;
    onChange: (value: string) => void;
    placeholder: string;
  }) => (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1">
        {label}
      </label>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
      />
    </div>
  ),
);

FilterInput.displayName = "FilterInput";

// Status options moved outside component to prevent recreation
const STATUS_OPTIONS = [
  { value: EnforcementStatus.CREATED, label: "Created" },
  { value: EnforcementStatus.PROCESSED, label: "Processed" },
  { value: EnforcementStatus.DISABLED, label: "Disabled" },
];

const getStatusColor = (status: EnforcementStatus) => {
  switch (status) {
    case EnforcementStatus.PROCESSED:
      return "bg-green-100 text-green-800";
    case EnforcementStatus.DISABLED:
      return "bg-gray-100 text-gray-800";
    case EnforcementStatus.CREATED:
      return "bg-blue-100 text-blue-800";
    default:
      return "bg-gray-100 text-gray-800";
  }
};

const getConditionBadge = (condition: string) => {
  const colors: Record<string, string> = {
    all: "bg-purple-100 text-purple-800",
    any: "bg-indigo-100 text-indigo-800",
  };
  return colors[condition] || "bg-gray-100 text-gray-800";
};

const formatTime = (timestamp: string) => {
  const date = new Date(timestamp);
  const now = new Date();
  const diff = now.getTime() - date.getTime();

  if (diff < 60000) return "just now";
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
  if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
  return date.toLocaleDateString();
};

const formatDate = (dateString: string) => {
  return new Date(dateString).toLocaleString();
};

// Memoized results table component - only re-renders when query data changes,
// NOT when the user types in filter inputs.
const EnforcementsResultsTable = memo(
  ({
    enforcements,
    isLoading,
    isFetching,
    error,
    hasActiveFilters,
    clearFilters,
  }: {
    enforcements: EnforcementSummary[];
    isLoading: boolean;
    isFetching: boolean;
    error: Error | null;
    hasActiveFilters: boolean;
    clearFilters: () => void;
  }) => {
    // Initial load (no cached data yet)
    if (isLoading && enforcements.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg">
          <div className="flex items-center justify-center h-64">
            <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
          </div>
        </div>
      );
    }

    // Error with no cached data to show
    if (error && enforcements.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg">
          <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
            <p>Error: {error.message}</p>
          </div>
        </div>
      );
    }

    // Empty results
    if (enforcements.length === 0) {
      return (
        <div className="bg-white p-12 text-center rounded-lg shadow">
          <p>No enforcements found</p>
          {hasActiveFilters && (
            <button
              onClick={clearFilters}
              className="mt-3 text-sm text-blue-600 hover:text-blue-800"
            >
              Clear filters
            </button>
          )}
        </div>
      );
    }

    return (
      <div className="relative">
        {/* Inline loading overlay - shows on top of previous results while fetching */}
        {isFetching && (
          <div className="absolute inset-0 bg-white/60 z-10 flex items-center justify-center rounded-lg">
            <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600" />
          </div>
        )}

        {/* Non-fatal error banner (data still shown from cache) */}
        {error && (
          <div className="mb-4 bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
            <p>Error refreshing: {error.message}</p>
          </div>
        )}

        <div className="bg-white shadow rounded-lg overflow-hidden">
          <table className="min-w-full">
            <thead className="bg-gray-50">
              <tr>
                <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  ID
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  Rule
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  Trigger
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  Event
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  Condition
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  Status
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase">
                  Created
                </th>
              </tr>
            </thead>
            <tbody className="bg-white divide-y divide-gray-200">
              {enforcements.map((enforcement: EnforcementSummary) => (
                <tr key={enforcement.id} className="hover:bg-gray-50">
                  <td className="px-6 py-4 font-mono text-sm">
                    <Link
                      to={`/enforcements/${enforcement.id}`}
                      className="text-blue-600 hover:text-blue-800"
                    >
                      #{enforcement.id}
                    </Link>
                  </td>
                  <td className="px-6 py-4">
                    <div className="flex items-center gap-2">
                      <PackIcon
                        packRef={packRefFromComponentRef(enforcement.rule_ref)}
                        size="sm"
                      />
                      {enforcement.rule ? (
                        <Link
                          to={`/rules/${enforcement.rule}`}
                          className="text-sm text-blue-600 hover:text-blue-800"
                        >
                          {enforcement.rule_ref}
                        </Link>
                      ) : (
                        <span className="text-sm text-gray-900">
                          {enforcement.rule_ref}
                        </span>
                      )}
                    </div>
                  </td>
                  <td className="px-6 py-4">
                    <div className="flex items-center gap-2">
                      <PackIcon
                        packRef={packRefFromComponentRef(
                          enforcement.trigger_ref,
                        )}
                        size="sm"
                      />
                      <span className="text-sm text-gray-700">
                        {enforcement.trigger_ref}
                      </span>
                    </div>
                  </td>
                  <td className="px-6 py-4">
                    {enforcement.event ? (
                      <Link
                        to={`/events/${enforcement.event}`}
                        className="text-sm font-mono text-blue-600 hover:text-blue-800"
                      >
                        #{enforcement.event}
                      </Link>
                    ) : (
                      <span className="text-sm text-gray-400 italic">-</span>
                    )}
                  </td>
                  <td className="px-6 py-4">
                    <span
                      className={`px-2 py-1 text-xs rounded ${getConditionBadge(enforcement.condition)}`}
                    >
                      {enforcement.condition}
                    </span>
                  </td>
                  <td className="px-6 py-4">
                    <span
                      className={`px-2 py-1 text-xs rounded ${getStatusColor(enforcement.status)}`}
                    >
                      {enforcement.status}
                    </span>
                  </td>
                  <td className="px-6 py-4">
                    <div className="text-sm text-gray-900">
                      {formatTime(enforcement.created)}
                    </div>
                    <div className="text-xs text-gray-500">
                      {formatDate(enforcement.created)}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    );
  },
);

EnforcementsResultsTable.displayName = "EnforcementsResultsTable";

export default function EnforcementsPage() {
  const [searchParams] = useSearchParams();

  // --- Filter input state (updates immediately on keystroke) ---
  const [page, setPage] = useState(1);
  const pageSize = 50;
  const [searchFilters, setSearchFilters] = useState({
    rule: searchParams.get("rule_ref") || "",
    trigger: searchParams.get("trigger_ref") || "",
    event: searchParams.get("event") || "",
  });
  const [selectedStatuses, setSelectedStatuses] = useState<string[]>(() => {
    const status = searchParams.get("status");
    return status ? [status] : [];
  });
  const [livePaused, setLivePaused] = useState(false);

  // --- Debounced filter state (drives API calls, updates after delay) ---
  const [debouncedFilters, setDebouncedFilters] = useState(searchFilters);
  const [debouncedStatuses, setDebouncedStatuses] = useState(selectedStatuses);

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedFilters(searchFilters);
    }, 500);
    return () => clearTimeout(timer);
  }, [searchFilters]);

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedStatuses(selectedStatuses);
    }, 300);
    return () => clearTimeout(timer);
  }, [selectedStatuses]);

  // --- Autocomplete suggestions ---
  const baseSuggestions = useFilterSuggestions();

  // --- Build query params from debounced state ---
  const queryParams = useMemo(() => {
    const params: {
      page: number;
      pageSize: number;
      triggerRef?: string;
      event?: number;
      status?: EnforcementStatus;
    } = { page, pageSize };
    if (debouncedFilters.trigger) params.triggerRef = debouncedFilters.trigger;
    if (debouncedFilters.event) {
      const eventId = parseInt(debouncedFilters.event, 10);
      if (!isNaN(eventId)) {
        params.event = eventId;
      }
    }
    if (debouncedStatuses.length === 1) {
      params.status = debouncedStatuses[0] as EnforcementStatus;
    }
    return params;
  }, [page, pageSize, debouncedFilters, debouncedStatuses]);

  const { data, isLoading, isFetching, error } = useEnforcements(queryParams);
  const { isConnected } = useEnforcementStream({
    enabled: true,
    paused: livePaused,
    maxListItems: DEFAULT_LIVE_LIST_MAX_ITEMS,
  });

  const enforcements = useMemo(() => data?.items || [], [data]);
  const total = data?.pagination?.total_items ?? undefined;
  const hasNext = data?.pagination?.has_next ?? false;
  const hasPrevious = data?.pagination?.has_previous ?? page > 1;

  // Derive refs from currently-loaded enforcement data (no setState needed)
  const loadedRefs = useMemo(() => {
    const rules = new Set<string>();
    const triggers = new Set<string>();

    for (const enf of enforcements) {
      if (enf.rule_ref) rules.add(enf.rule_ref as string);
      if (enf.trigger_ref) triggers.add(enf.trigger_ref as string);
    }

    return {
      rules: [...rules],
      triggers: [...triggers],
    };
  }, [enforcements]);

  // Merge base entity suggestions + loaded data refs
  const ruleSuggestions = useMergedSuggestions(
    baseSuggestions.ruleRefs,
    loadedRefs.rules,
  );
  const triggerSuggestions = useMergedSuggestions(
    baseSuggestions.triggerRefs,
    loadedRefs.triggers,
  );

  // Client-side filtering for rule_ref and multiple status selection
  const filteredEnforcements = useMemo(() => {
    let filtered = enforcements;

    // Filter by rule_ref (client-side since API doesn't support it)
    if (debouncedFilters.rule) {
      filtered = filtered.filter((enf: EnforcementSummary) =>
        enf.rule_ref
          ?.toLowerCase()
          .includes(debouncedFilters.rule.toLowerCase()),
      );
    }

    // If multiple statuses selected, filter client-side
    if (debouncedStatuses.length > 1) {
      filtered = filtered.filter((enf: EnforcementSummary) =>
        debouncedStatuses.includes(enf.status),
      );
    }

    return filtered;
  }, [enforcements, debouncedFilters.rule, debouncedStatuses]);

  const handleFilterChange = useCallback((field: string, value: string) => {
    setSearchFilters((prev) => ({ ...prev, [field]: value }));
    setPage(1);
  }, []);

  const clearFilters = useCallback(() => {
    setSearchFilters({ rule: "", trigger: "", event: "" });
    setSelectedStatuses([]);
    setPage(1);
  }, []);

  const hasActiveFilters =
    Object.values(searchFilters).some((v) => v !== "") ||
    selectedStatuses.length > 0;

  return (
    <div className="p-6 pb-28">
      {/* Header - always visible */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-3xl font-bold">Enforcements</h1>
          {isFetching && hasActiveFilters && (
            <p className="text-sm text-gray-500 mt-1">
              Searching enforcements...
            </p>
          )}
        </div>
        <LiveStreamControl
          paused={livePaused}
          onTogglePaused={() => setLivePaused((paused) => !paused)}
          connected={isConnected}
          maxItems={DEFAULT_LIVE_LIST_MAX_ITEMS}
          itemLabel="enforcements"
        />
      </div>

      {/* Filter section - always mounted, never unmounts during loading */}
      <div className="bg-white shadow rounded-lg p-4 mb-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <Search className="h-5 w-5 text-gray-400" />
            <h2 className="text-lg font-semibold">Filter Enforcements</h2>
          </div>
          {hasActiveFilters && (
            <button
              onClick={clearFilters}
              className="flex items-center gap-1 text-sm text-gray-600 hover:text-gray-900"
            >
              <X className="h-4 w-4" />
              Clear Filters
            </button>
          )}
        </div>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          <AutocompleteInput
            label="Rule"
            value={searchFilters.rule}
            onChange={(value) => handleFilterChange("rule", value)}
            suggestions={ruleSuggestions}
            placeholder="e.g., core.on_timer"
          />
          <AutocompleteInput
            label="Trigger"
            value={searchFilters.trigger}
            onChange={(value) => handleFilterChange("trigger", value)}
            suggestions={triggerSuggestions}
            placeholder="e.g., core.webhook"
          />
          <FilterInput
            label="Event ID"
            value={searchFilters.event}
            onChange={(value) => handleFilterChange("event", value)}
            placeholder="e.g., 123"
          />
          <div>
            <MultiSelect
              label="Status"
              options={STATUS_OPTIONS}
              value={selectedStatuses}
              onChange={setSelectedStatuses}
              placeholder="All Statuses"
            />
          </div>
        </div>
      </div>

      {/* Results section - isolated from filter state, only depends on query results */}
      <EnforcementsResultsTable
        enforcements={filteredEnforcements}
        isLoading={isLoading}
        isFetching={isFetching}
        error={error as Error | null}
        hasActiveFilters={hasActiveFilters}
        clearFilters={clearFilters}
      />

      <Pagination
        page={page}
        setPage={setPage}
        pageSize={pageSize}
        itemCount={filteredEnforcements.length}
        total={total}
        hasNext={hasNext}
        hasPrevious={hasPrevious}
        itemLabel="enforcements"
        floating
      />
    </div>
  );
}
