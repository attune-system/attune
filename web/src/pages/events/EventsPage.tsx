import { useState, useCallback, useMemo, memo, useEffect } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { useEvents } from "@/hooks/useEvents";
import {
  useEntityNotifications,
  Notification,
} from "@/contexts/WebSocketContext";
import { Search, X } from "lucide-react";
import LiveStreamControl, {
  DEFAULT_LIVE_LIST_MAX_ITEMS,
} from "@/components/common/LiveStreamControl";
import AutocompleteInput from "@/components/common/AutocompleteInput";
import {
  useFilterSuggestions,
  useMergedSuggestions,
} from "@/hooks/useFilterSuggestions";
import { matchesRefFilter, packPrefix } from "@/utils/refFilters";
import type { EventSummary } from "@/api";
import Pagination from "@/components/executions/Pagination";
import PackIcon from "@/components/common/PackIcon";
import { packRefFromComponentRef } from "@/utils/packIcons";

const formatDate = (dateString: string) => {
  return new Date(dateString).toLocaleString();
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

// Memoized results table component - only re-renders when query data changes,
// NOT when the user types in filter inputs.
const EventsResultsTable = memo(
  ({
    events,
    isLoading,
    isFetching,
    error,
    hasActiveFilters,
    clearFilters,
  }: {
    events: EventSummary[];
    isLoading: boolean;
    isFetching: boolean;
    error: Error | null;
    hasActiveFilters: boolean;
    clearFilters: () => void;
  }) => {
    // Initial load (no cached data yet)
    if (isLoading && events.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg">
          <div className="flex items-center justify-center h-64">
            <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
            <p className="ml-4 text-gray-600">Loading events...</p>
          </div>
        </div>
      );
    }

    // Error with no cached data to show
    if (error && events.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg p-12 text-center">
          <p className="text-red-600">Failed to load events</p>
          <p className="text-sm text-gray-600 mt-2">{error.message}</p>
        </div>
      );
    }

    // Empty results
    if (events.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg p-12 text-center">
          <svg
            className="mx-auto h-12 w-12 text-gray-400"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth={2}
              d="M13 10V3L4 14h7v7l9-11h-7z"
            />
          </svg>
          <p className="mt-4 text-gray-600">No events found</p>
          <p className="text-sm text-gray-500 mt-1">
            {hasActiveFilters
              ? "Try adjusting your filters"
              : "Events will appear here when triggers fire"}
          </p>
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
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-200">
              <thead className="bg-gray-50">
                <tr>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    ID
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Trigger
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Rule
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Source
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Created
                  </th>
                </tr>
              </thead>
              <tbody className="bg-white divide-y divide-gray-200">
                {events.map((event) => (
                  <tr key={event.id} className="hover:bg-gray-50">
                    <td className="px-6 py-4 whitespace-nowrap">
                      <Link
                        to={`/events/${event.id}`}
                        className="text-sm font-mono text-blue-600 hover:text-blue-800"
                      >
                        #{event.id}
                      </Link>
                    </td>
                    <td className="px-6 py-4">
                      <div className="flex items-start gap-2 text-sm">
                        <PackIcon
                          packRef={packRefFromComponentRef(event.trigger_ref)}
                          size="sm"
                          className="mt-0.5"
                        />
                        <div className="min-w-0">
                          <div className="font-medium text-gray-900 truncate">
                            {event.trigger_ref}
                          </div>
                          <div className="text-gray-500 text-xs">
                            ID: {event.trigger || "N/A"}
                          </div>
                        </div>
                      </div>
                    </td>
                    <td className="px-6 py-4">
                      {event.rule_ref ? (
                        <div className="text-sm">
                          <Link
                            to={`/rules/${event.rule}`}
                            className="font-medium text-blue-600 hover:text-blue-900"
                          >
                            {event.rule_ref}
                          </Link>
                          <div className="text-gray-500 text-xs">
                            ID: {event.rule}
                          </div>
                        </div>
                      ) : (
                        <span className="text-sm text-gray-400 italic">
                          No rule
                        </span>
                      )}
                    </td>
                    <td className="px-6 py-4">
                      {event.source_ref ? (
                        <div className="text-sm">
                          <div className="font-medium text-gray-900">
                            {event.source_ref}
                          </div>
                          <div className="text-gray-500 text-xs">
                            ID: {event.source || "N/A"}
                          </div>
                        </div>
                      ) : (
                        <span className="text-sm text-gray-400 italic">
                          No source
                        </span>
                      )}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap">
                      <div className="text-sm text-gray-900">
                        {formatTime(event.created)}
                      </div>
                      <div className="text-xs text-gray-500">
                        {formatDate(event.created)}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    );
  },
);

EventsResultsTable.displayName = "EventsResultsTable";

export default function EventsPage() {
  const [searchParams] = useSearchParams();
  const queryClient = useQueryClient();

  // --- Filter input state (updates immediately on keystroke) ---
  const [page, setPage] = useState(1);
  const pageSize = 50;
  const [searchFilters, setSearchFilters] = useState({
    trigger: searchParams.get("trigger_ref") || "",
    rule: searchParams.get("rule_ref") || "",
  });

  // --- Debounced filter state (drives API calls, updates after delay) ---
  const [debouncedFilters, setDebouncedFilters] = useState(searchFilters);

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedFilters(searchFilters);
    }, 500);
    return () => clearTimeout(timer);
  }, [searchFilters]);

  // --- Autocomplete suggestions ---
  const baseSuggestions = useFilterSuggestions();

  const [livePaused, setLivePaused] = useState(false);

  // Additional refs discovered via WebSocket notifications (accumulated over time)
  const [wsRefs, setWsRefs] = useState<{
    triggers: string[];
    rules: string[];
  }>({ triggers: [], rules: [] });

  // --- Build query params from debounced state ---
  const queryParams = useMemo(() => {
    const params: {
      page: number;
      pageSize: number;
      triggerRef?: string;
      ruleRef?: string;
    } = { page, pageSize };
    if (debouncedFilters.trigger) params.triggerRef = debouncedFilters.trigger;
    if (debouncedFilters.rule) params.ruleRef = debouncedFilters.rule;
    return params;
  }, [page, pageSize, debouncedFilters]);

  // Set up WebSocket for real-time event updates with stable callback
  const handleEventNotification = useCallback(
    (notification: Notification) => {
      if (livePaused) {
        return;
      }

      if (notification.notification_type === "event_created") {
        const payload = notification.payload as Partial<EventSummary> & {
          payload?: unknown;
          has_payload?: boolean;
        };

        const newEvent: EventSummary = {
          id: payload.id ?? 0,
          trigger: payload.trigger ?? 0,
          trigger_ref: payload.trigger_ref ?? "",
          rule: payload.rule,
          rule_ref: payload.rule_ref,
          source: payload.source,
          source_ref: payload.source_ref,
          payload: payload.payload ?? null,
          has_payload:
            payload.has_payload ??
            (payload.payload !== null && payload.payload !== undefined),
          created: payload.created ?? new Date().toISOString(),
        };

        // Augment autocomplete suggestions with new refs from notification
        setWsRefs((prev) => {
          const newTriggers = new Set(prev.triggers);
          const newRules = new Set(prev.rules);
          let changed = false;

          if (newEvent.trigger_ref && !newTriggers.has(newEvent.trigger_ref)) {
            newTriggers.add(newEvent.trigger_ref);
            changed = true;
          }
          if (newEvent.rule_ref && !newRules.has(newEvent.rule_ref)) {
            newRules.add(newEvent.rule_ref);
            changed = true;
          }

          if (!changed) return prev;
          return {
            triggers: [...newTriggers],
            rules: [...newRules],
          };
        });

        queryClient.setQueryData(
          ["events", queryParams],
          (
            oldData:
              | {
                  items: EventSummary[];
                  pagination?: {
                    total_items?: number;
                    total_pages?: number;
                    page?: number;
                    page_size?: number;
                    has_previous?: boolean;
                    has_next?: boolean;
                  };
                }
              | undefined,
          ) => {
            if (!oldData) return oldData;

            // Check if filtering and event matches filter
            if (
              debouncedFilters.trigger &&
              !matchesRefFilter(newEvent.trigger_ref, debouncedFilters.trigger)
            ) {
              return oldData;
            }
            if (
              debouncedFilters.rule &&
              !matchesRefFilter(newEvent.rule_ref, debouncedFilters.rule)
            ) {
              return oldData;
            }

            const hasExactTotal = oldData.pagination?.total_items != null;
            const currentPage = oldData.pagination?.page ?? page;
            const currentPageSize = oldData.pagination?.page_size ?? pageSize;
            const nextPagination = oldData.pagination
              ? { ...oldData.pagination }
              : undefined;

            if (nextPagination) {
              nextPagination.has_previous = currentPage > 1;
            }

            // Add new event to the beginning of the list if on first page
            if (page === 1) {
              if (nextPagination) {
                if (hasExactTotal) {
                  const newTotal = (oldData.pagination?.total_items ?? 0) + 1;
                  nextPagination.total_items = newTotal;
                  nextPagination.total_pages =
                    currentPageSize > 0
                      ? Math.ceil(newTotal / currentPageSize)
                      : 0;
                  nextPagination.has_next =
                    currentPage * currentPageSize < newTotal;
                } else if (oldData.items.length >= currentPageSize) {
                  nextPagination.has_next = true;
                }
              }
              return {
                ...oldData,
                items: [newEvent, ...oldData.items].slice(
                  0,
                  DEFAULT_LIVE_LIST_MAX_ITEMS,
                ),
                pagination: nextPagination,
              };
            }

            if (nextPagination && hasExactTotal) {
              const newTotal = (oldData.pagination?.total_items ?? 0) + 1;
              nextPagination.total_items = newTotal;
              nextPagination.total_pages =
                currentPageSize > 0 ? Math.ceil(newTotal / currentPageSize) : 0;
              nextPagination.has_next =
                currentPage * currentPageSize < newTotal;
            }

            return {
              ...oldData,
              pagination: nextPagination,
            };
          },
        );
      }
    },
    [queryClient, queryParams, page, pageSize, debouncedFilters, livePaused],
  );

  const { connected: wsConnected } = useEntityNotifications(
    "event",
    handleEventNotification,
    !livePaused,
  );

  const { data, isLoading, isFetching, error } = useEvents(queryParams);

  const events = useMemo(() => data?.items || [], [data]);
  const total = data?.pagination?.total_items ?? undefined;
  const hasNext = data?.pagination?.has_next ?? false;
  const hasPrevious = data?.pagination?.has_previous ?? page > 1;

  // Derive refs from currently-loaded event data (no setState needed)
  const loadedRefs = useMemo(() => {
    const triggers = new Set<string>();
    const rules = new Set<string>();

    for (const event of events) {
      if (event.trigger_ref) {
        triggers.add(event.trigger_ref);
      }
      if (event.rule_ref) {
        rules.add(event.rule_ref);
      }
    }

    return {
      triggers: [...triggers],
      rules: [...rules],
    };
  }, [events]);

  const wildcardSuggestions = useMemo(() => {
    const wildcards = new Set<string>();
    const refs = [
      ...loadedRefs.triggers,
      ...loadedRefs.rules,
      ...wsRefs.triggers,
      ...wsRefs.rules,
    ];

    for (const pack of baseSuggestions.packNames) {
      if (pack) wildcards.add(`${pack}.*`);
    }
    for (const ref of refs) {
      const pack = packPrefix(ref);
      if (pack) wildcards.add(`${pack}.*`);
    }

    return [...wildcards].sort();
  }, [
    baseSuggestions.packNames,
    loadedRefs.triggers,
    loadedRefs.rules,
    wsRefs.triggers,
    wsRefs.rules,
  ]);

  // Merge base entity suggestions + wildcard pack refs + loaded data refs + WebSocket refs
  const triggerSuggestions = useMergedSuggestions(
    baseSuggestions.triggerRefs,
    wildcardSuggestions,
    loadedRefs.triggers,
    wsRefs.triggers,
  );
  const ruleSuggestions = useMergedSuggestions(
    baseSuggestions.ruleRefs,
    wildcardSuggestions,
    loadedRefs.rules,
    wsRefs.rules,
  );

  const handleFilterChange = useCallback((field: string, value: string) => {
    setSearchFilters((prev) => ({ ...prev, [field]: value }));
    setPage(1);
  }, []);

  const clearFilters = useCallback(() => {
    setSearchFilters({ trigger: "", rule: "" });
    setPage(1);
  }, []);

  const hasActiveFilters = Object.values(searchFilters).some((v) => v !== "");

  return (
    <div className="p-6 pb-28">
      {/* Header - always visible */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-3xl font-bold text-gray-900">Events</h1>
            <p className="mt-2 text-gray-600">
              Event instances generated by sensors and triggers
            </p>
            {isFetching && hasActiveFilters && (
              <p className="text-sm text-gray-500 mt-1">Searching events...</p>
            )}
          </div>
          <LiveStreamControl
            paused={livePaused}
            onTogglePaused={() => setLivePaused((paused) => !paused)}
            connected={wsConnected}
            maxItems={DEFAULT_LIVE_LIST_MAX_ITEMS}
            itemLabel="events"
          />
        </div>
      </div>

      {/* Filter section - always mounted, never unmounts during loading */}
      <div className="bg-white shadow rounded-lg p-4 mb-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <Search className="h-5 w-5 text-gray-400" />
            <h2 className="text-lg font-semibold">Filter Events</h2>
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
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <AutocompleteInput
            label="Trigger"
            value={searchFilters.trigger}
            onChange={(value) => handleFilterChange("trigger", value)}
            suggestions={triggerSuggestions}
            placeholder="e.g., core.webhook, core.*, or core.queue_*"
          />
          <AutocompleteInput
            label="Rule"
            value={searchFilters.rule}
            onChange={(value) => handleFilterChange("rule", value)}
            suggestions={ruleSuggestions}
            placeholder="e.g., core.on_webhook, core.*, or core.queue_*"
          />
        </div>
        {data && (
          <div className="mt-3 text-sm text-gray-600">
            {typeof total === "number"
              ? `Showing ${events.length} of ${total} events`
              : hasNext
                ? `Showing ${events.length} events, more available`
                : `Showing ${events.length} events`}
            {hasActiveFilters ? " (filtered)" : ""}
          </div>
        )}
      </div>

      {/* Results section - isolated from filter state, only depends on query results */}
      <EventsResultsTable
        events={events}
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
        itemCount={events.length}
        total={total}
        hasNext={hasNext}
        hasPrevious={hasPrevious}
        itemLabel="events"
        floating
      />
    </div>
  );
}
