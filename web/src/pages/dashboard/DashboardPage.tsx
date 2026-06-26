import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { DashboardCard } from "@/components/dashboard/DashboardCard";
import {
  getDashboardFilterDefaults,
  normalizeDashboardDataRequest,
  useDashboardData,
  useDashboardSpec,
} from "@/hooks/useDashboards";
import type {
  DashboardDataRequest,
  DashboardFilterSpec,
  DashboardFilterValue,
  DashboardSourceResult,
} from "@/types/dashboard";

const DEFAULT_DASHBOARD_REF =
  import.meta.env.VITE_DASHBOARD_REF || "core.operations";

function useViewportWidth(): number {
  const [width, setWidth] = useState<number>(() => window.innerWidth);

  useEffect(() => {
    const handler = () => setWidth(window.innerWidth);
    window.addEventListener("resize", handler);
    return () => window.removeEventListener("resize", handler);
  }, []);

  return width;
}

function parseFilterValue(
  filter: DashboardFilterSpec,
  value: string,
): DashboardFilterValue {
  if (filter.type === "number") {
    const numeric = Number(value);
    return Number.isFinite(numeric) ? numeric : value;
  }
  if (filter.type === "boolean") {
    return value === "true";
  }
  return value;
}

function FilterControl({
  filter,
  value,
  onChange,
}: {
  filter: DashboardFilterSpec;
  value: DashboardFilterValue | undefined;
  onChange: (next: DashboardFilterValue | undefined) => void;
}) {
  const options = filter.options;

  if (filter.type === "boolean") {
    const checked = value === true;
    return (
      <label className="flex items-center gap-2 text-sm text-gray-700">
        <input
          type="checkbox"
          checked={checked}
          onChange={(event) => onChange(event.target.checked)}
        />
        {filter.label}
      </label>
    );
  }

  const isMulti = Array.isArray(value) || Array.isArray(filter.default);

  if (options?.length) {
    if (isMulti) {
      const current = Array.isArray(value) ? value.map(String) : [];
      return (
        <label className="text-sm text-gray-700 flex flex-col gap-1">
          <span>{filter.label}</span>
          <select
            multiple
            value={current}
            onChange={(event) => {
              const selected = Array.from(event.target.selectedOptions).map(
                (option) => parseFilterValue(filter, option.value),
              );
              onChange(selected as DashboardFilterValue);
            }}
            className="border border-gray-300 rounded px-2 py-1 min-w-44"
          >
            {options.map((option, idx) => (
              <option key={`${filter.id}-${idx}`} value={String(option)}>
                {String(option)}
              </option>
            ))}
          </select>
        </label>
      );
    }

    return (
      <label className="text-sm text-gray-700 flex flex-col gap-1">
        <span>{filter.label}</span>
        <select
          value={value === undefined || value === null ? "" : String(value)}
          onChange={(event) => {
            if (!event.target.value) {
              onChange(undefined);
              return;
            }
            onChange(parseFilterValue(filter, event.target.value));
          }}
          className="border border-gray-300 rounded px-2 py-1 min-w-44"
        >
          <option value="">All</option>
          {options.map((option, idx) => (
            <option key={`${filter.id}-${idx}`} value={String(option)}>
              {String(option)}
            </option>
          ))}
        </select>
      </label>
    );
  }

  return (
    <label className="text-sm text-gray-700 flex flex-col gap-1">
      <span>{filter.label}</span>
      <input
        type={filter.type === "number" ? "number" : "text"}
        value={value === undefined || value === null ? "" : String(value)}
        onChange={(event) => {
          if (!event.target.value) {
            onChange(undefined);
            return;
          }
          onChange(parseFilterValue(filter, event.target.value));
        }}
        className="border border-gray-300 rounded px-2 py-1 min-w-44"
      />
    </label>
  );
}

export default function DashboardPage() {
  const queryClient = useQueryClient();
  const width = useViewportWidth();
  const [searchParams] = useSearchParams();

  const dashboardRef = searchParams.get("ref") || DEFAULT_DASHBOARD_REF;

  const { data: spec, isLoading: specLoading, error: specError } =
    useDashboardSpec(dashboardRef);

  const [filterOverrides, setFilterOverrides] = useState<
    Record<string, DashboardFilterValue | null>
  >({});
  const [timeWindowOverride, setTimeWindowOverride] = useState<
    string | null | undefined
  >(undefined);
  const [timezoneOverride, setTimezoneOverride] = useState<string | undefined>(
    undefined,
  );

  const filters = useMemo(() => {
    const base = getDashboardFilterDefaults(spec);
    for (const [key, value] of Object.entries(filterOverrides)) {
      if (value === null) {
        delete base[key];
      } else {
        base[key] = value;
      }
    }
    return base;
  }, [spec, filterOverrides]);

  const timeWindow =
    timeWindowOverride === undefined
      ? spec?.defaults?.time_window
      : timeWindowOverride || undefined;
  const timezone = timezoneOverride ?? spec?.defaults?.timezone ?? "UTC";

  const request = useMemo<DashboardDataRequest>(() => {
    const next: DashboardDataRequest = {
      filters,
      time_window: timeWindow,
      timezone,
      card_ids: spec?.cards.map((card) => card.id),
      include_meta: true,
    };

    return normalizeDashboardDataRequest(next);
  }, [filters, timeWindow, timezone, spec?.cards]);

  const {
    data: dataResponse,
    isLoading: dataLoading,
    isFetching: dataFetching,
    error: dataError,
    refetch,
  } = useDashboardData({
    dashboardRef,
    request,
    enabled: Boolean(spec),
    refreshSeconds: spec?.defaults?.refresh_seconds,
  });

  const sourceById = useMemo(() => {
    const map = new Map<string, DashboardSourceResult>();
    for (const source of dataResponse?.sources || []) {
      map.set(source.source_id, source);
    }
    return map;
  }, [dataResponse?.sources]);

  const activeBreakpoint = useMemo(() => {
    if (!spec) return "lg";
    const ordered = Object.entries(spec.layout.breakpoints).sort(
      (a, b) => b[1].min_width - a[1].min_width,
    );
    const match = ordered.find(([, bp]) => width >= bp.min_width);
    return match?.[0] || ordered[ordered.length - 1]?.[0] || "lg";
  }, [spec, width]);

  const activeColumns =
    spec?.layout.breakpoints[activeBreakpoint]?.columns || spec?.layout.columns || 12;

  if (specLoading) {
    return <div className="p-6 text-sm text-gray-600">Loading dashboard…</div>;
  }

  if (specError || !spec) {
    return (
      <div className="p-6">
        <h1 className="text-xl font-semibold text-gray-900">Dashboard</h1>
        <p className="mt-2 text-sm text-red-600">
          Failed to load dashboard spec for <code>{dashboardRef}</code>.
        </p>
      </div>
    );
  }

  return (
    <div className="p-6 space-y-4">
      <header className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-bold text-gray-900">{spec.label}</h1>
          {spec.description && (
            <p className="mt-1 text-sm text-gray-600">{spec.description}</p>
          )}
          <p className="mt-1 text-xs text-gray-500">
            ref: {spec.ref} • revision: {dataResponse?.dashboard_revision ?? spec.revision ?? "—"}
          </p>
        </div>
        <button
          type="button"
          onClick={() => refetch()}
          className="px-3 py-1.5 rounded border border-gray-300 bg-white text-sm text-gray-700 hover:bg-gray-50"
          disabled={dataFetching}
        >
          {dataFetching ? "Refreshing…" : "Refresh"}
        </button>
      </header>

      <section className="bg-white border border-gray-200 rounded-lg p-3 flex flex-wrap gap-3 items-end">
        {(spec.filters || []).map((filter) => (
          <FilterControl
            key={filter.id}
            filter={filter}
            value={filters[filter.id]}
            onChange={(next) => {
              setFilterOverrides((current) => ({
                ...current,
                [filter.id]:
                  next === undefined || next === null || next === ""
                    ? null
                    : next,
              }));
            }}
          />
        ))}

        <label className="text-sm text-gray-700 flex flex-col gap-1">
          <span>Time Window</span>
          <select
            value={timeWindow || ""}
            onChange={(event) =>
              setTimeWindowOverride(
                event.target.value ? event.target.value : null,
              )
            }
            className="border border-gray-300 rounded px-2 py-1 min-w-32"
          >
            {["", "15m", "1h", "6h", "24h", "7d"].map((option) => (
              <option key={option || "default"} value={option}>
                {option || "Default"}
              </option>
            ))}
          </select>
        </label>

        <label className="text-sm text-gray-700 flex flex-col gap-1">
          <span>Timezone</span>
          <input
            value={timezone}
            onChange={(event) =>
              setTimezoneOverride(event.target.value || undefined)
            }
            className="border border-gray-300 rounded px-2 py-1 min-w-44"
            placeholder="UTC"
          />
        </label>

        <button
          type="button"
          onClick={() => {
            setFilterOverrides({});
            setTimeWindowOverride(undefined);
            setTimezoneOverride(undefined);
            queryClient.removeQueries({ queryKey: ["dashboards", dashboardRef, "data"] });
          }}
          className="px-3 py-1.5 rounded text-sm border border-gray-300 text-gray-600 hover:bg-gray-50"
        >
          Reset
        </button>
      </section>

      {dataError && (
        <div className="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          Failed to load dashboard data. Request-level errors return here; source-level errors render per card.
        </div>
      )}

      {dataResponse?.partial && (
        <div className="rounded border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">
          Partial dashboard data: one or more sources reported non-OK status.
        </div>
      )}

      {dataLoading && !dataResponse ? (
        <div className="text-sm text-gray-600">Loading card data…</div>
      ) : (
        <section
          className="grid"
          style={{
            gridTemplateColumns: `repeat(${activeColumns}, minmax(0, 1fr))`,
            gap: `${spec.layout.gap}px`,
            gridAutoRows: `${spec.layout.row_height}px`,
          }}
        >
          {spec.cards.map((card) => {
            const rect =
              card.position[activeBreakpoint] ||
              card.position.lg ||
              Object.values(card.position)[0];

            return (
              <div
                key={card.id}
                style={{
                  gridColumn: `${rect.x + 1} / span ${rect.w}`,
                  gridRow: `${rect.y + 1} / span ${rect.h}`,
                }}
              >
                <DashboardCard
                  card={card}
                  source={sourceById.get(card.source)}
                  isRefreshing={dataFetching}
                  onRetry={() => refetch()}
                />
              </div>
            );
          })}
        </section>
      )}
    </div>
  );
}
