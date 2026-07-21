import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Link,
  useNavigate,
  useParams,
  useSearchParams,
} from "react-router-dom";
import {
  Bot,
  Code2,
  Cpu,
  Pencil,
  Plus,
  Search,
  Server,
  Trash2,
  X,
} from "lucide-react";

import type { RuntimeSummary } from "@/api";
import type { WorkerRuntimeSupport, WorkerSummary } from "@/api/workers";
import RuntimeForm from "@/components/forms/RuntimeForm";
import { useAuth } from "@/contexts/AuthContext";
import {
  useInfiniteRuntimes,
  useRuntime,
  useDeleteRuntime,
} from "@/hooks/useRuntimes";
import {
  useCordonWorker,
  useUncordonWorker,
  useInfiniteWorkers,
} from "@/hooks/useWorkers";
import { hasPermission } from "@/lib/permissions";
import InfiniteScrollTrigger from "@/components/common/InfiniteScrollTrigger";
import { useDebouncedValue } from "@/hooks/useDebouncedValue";

function formatJson(value: unknown): string {
  return JSON.stringify(value ?? null, null, 2);
}

function normalizeRuntimeName(value: string): string {
  switch (value.trim().toLowerCase()) {
    case "node":
    case "nodejs":
    case "node.js":
      return "node";
    case "python":
    case "python3":
      return "python";
    case "shell":
    case "bash":
    case "sh":
      return "shell";
    case "native":
    case "builtin":
    case "standalone":
      return "native";
    case "golang":
      return "go";
    case "jdk":
    case "openjdk":
      return "java";
    case "perl5":
      return "perl";
    case "rscript":
      return "r";
    default:
      return value.trim().toLowerCase();
  }
}

function runtimeLabel(name: string): string {
  const normalized = normalizeRuntimeName(name);
  switch (normalized) {
    case "node":
      return "Node.js";
    case "python":
      return "Python";
    case "shell":
      return "Shell";
    case "native":
      return "Native";
    case "go":
      return "Go";
    case "java":
      return "Java";
    case "perl":
      return "Perl";
    case "ruby":
      return "Ruby";
    case "r":
      return "R";
    default:
      return normalized
        .split(/[\s._-]+/)
        .filter(Boolean)
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(" ");
  }
}

function formatDateTime(dateString?: string | null): string {
  if (!dateString) {
    return "—";
  }
  return new Date(dateString).toLocaleString();
}

function formatRelativeTime(dateString?: string | null): string {
  if (!dateString) {
    return "No heartbeat";
  }

  const diffMs = new Date(dateString).getTime() - Date.now();
  const diffMinutes = Math.round(diffMs / (1000 * 60));
  const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

  if (Math.abs(diffMinutes) < 60) {
    return formatter.format(diffMinutes, "minute");
  }

  const diffHours = Math.round(diffMinutes / 60);
  if (Math.abs(diffHours) < 24) {
    return formatter.format(diffHours, "hour");
  }

  const diffDays = Math.round(diffHours / 24);
  return formatter.format(diffDays, "day");
}

function getWorkerStatusClasses(status?: string | null): string {
  switch (status) {
    case "active":
      return "bg-emerald-50 text-emerald-700 border-emerald-200";
    case "busy":
      return "bg-amber-50 text-amber-700 border-amber-200";
    case "error":
      return "bg-red-50 text-red-700 border-red-200";
    default:
      return "bg-gray-50 text-gray-700 border-gray-200";
  }
}

function getUtilizationBarClasses(percent?: number | null): string {
  const value = percent ?? 0;
  if (value >= 90) {
    return "bg-red-500";
  }
  if (value >= 70) {
    return "bg-amber-500";
  }
  return "bg-blue-600";
}

type TabId = "workers" | "runtimes";

export default function RuntimesPage() {
  const { user } = useAuth();
  const { ref } = useParams<{ ref?: string }>();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const canReadWorkers = hasPermission(user, "workers");
  const canReadRuntimes = hasPermission(user, "runtimes");

  const tabParam = searchParams.get("tab");
  const activeTab: TabId =
    tabParam === "runtimes" || tabParam === "workers"
      ? tabParam
      : ref
        ? "runtimes"
        : "workers";
  const visibleTab: TabId =
    activeTab === "workers" && canReadWorkers
      ? "workers"
      : activeTab === "runtimes" && canReadRuntimes
        ? "runtimes"
        : canReadWorkers
          ? "workers"
          : "runtimes";

  useEffect(() => {
    if (activeTab === "workers" && !canReadWorkers) {
      navigate(canReadRuntimes ? "/runtimes?tab=runtimes" : "/", {
        replace: true,
      });
    } else if (activeTab === "runtimes" && !canReadRuntimes) {
      navigate(canReadWorkers ? "/runtimes?tab=workers" : "/", {
        replace: true,
      });
    }
  }, [activeTab, canReadRuntimes, canReadWorkers, navigate]);

  const setTab = (tab: TabId) => {
    if (tab === "workers") {
      navigate("/runtimes?tab=workers");
      return;
    }

    if (ref) {
      navigate(`/runtimes/${encodeURIComponent(ref)}?tab=runtimes`);
      return;
    }

    navigate("/runtimes?tab=runtimes");
  };

  return (
    <div className="flex h-full min-h-0 flex-col p-6">
      <div className="mb-6 shrink-0">
        <h1 className="text-3xl font-bold text-gray-900 flex items-center gap-3">
          <Server className="w-8 h-8 text-blue-600" />
          Runtimes & Workers
        </h1>
        <p className="mt-2 text-gray-600">
          Inspect worker capacity and runtime support, then manage runtime
          definitions.
        </p>
      </div>

      <div className="mb-6 shrink-0 border-b border-gray-200">
        <nav className="-mb-px flex space-x-8">
          {canReadWorkers && (
            <button
              onClick={() => setTab("workers")}
              className={`whitespace-nowrap py-3 px-1 border-b-2 font-medium text-sm transition-colors ${
                visibleTab === "workers"
                  ? "border-blue-500 text-blue-600"
                  : "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
              }`}
            >
              <div className="flex items-center gap-2">
                <Bot className="w-4 h-4" />
                Workers
              </div>
            </button>
          )}
          {canReadRuntimes && (
            <button
              onClick={() => setTab("runtimes")}
              className={`whitespace-nowrap py-3 px-1 border-b-2 font-medium text-sm transition-colors ${
                visibleTab === "runtimes"
                  ? "border-indigo-500 text-indigo-600"
                  : "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
              }`}
            >
              <div className="flex items-center gap-2">
                <Code2 className="w-4 h-4" />
                Runtimes
              </div>
            </button>
          )}
        </nav>
      </div>

      <div className="min-h-0 flex-1">
        {canReadWorkers || canReadRuntimes ? (
          visibleTab === "workers" ? (
            <WorkersTab canReadRuntimes={canReadRuntimes} />
          ) : (
            <RuntimesTab
              runtimeRef={ref}
              canCreateRuntime={hasPermission(user, "runtimes", "create")}
              canUpdateRuntime={hasPermission(user, "runtimes", "update")}
              canDeleteRuntime={hasPermission(user, "runtimes", "delete")}
            />
          )
        ) : (
          <PermissionNotice message="You do not have permission to view workers or runtimes." />
        )}
      </div>
    </div>
  );
}

function WorkersTab({ canReadRuntimes }: { canReadRuntimes: boolean }) {
  const [searchQuery, setSearchQuery] = useState("");
  const debouncedSearchQuery = useDebouncedValue(searchQuery.trim());
  const {
    data,
    isLoading,
    error,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
  } = useInfiniteWorkers({
    enabled: true,
    query: debouncedSearchQuery || undefined,
  });
  const { data: runtimeData } = useInfiniteRuntimes({
    enabled: canReadRuntimes,
  });
  const [runtimeFilter, setRuntimeFilter] = useState("all");
  const [roleFilter, setRoleFilter] = useState("all");
  const [showNonActiveWorkers, setShowNonActiveWorkers] = useState(false);
  const workerListRef = useRef<HTMLDivElement | null>(null);

  const workers = useMemo(
    () => data?.pages.flatMap((page) => page.items) ?? [],
    [data?.pages],
  );
  const totalWorkers = data?.pages[0]?.pagination.total_items ?? workers.length;
  const runtimes = useMemo(
    () => runtimeData?.pages.flatMap((page) => page.items) ?? [],
    [runtimeData?.pages],
  );
  const runtimeNames = useMemo(() => {
    const seen = new Set<string>();

    for (const runtime of runtimes) {
      const refName = runtime.ref.split(".").pop();
      if (refName) {
        seen.add(normalizeRuntimeName(refName));
      }
      seen.add(normalizeRuntimeName(runtime.name));
    }

    for (const worker of workers) {
      for (const runtime of worker.supported_runtimes) {
        seen.add(normalizeRuntimeName(runtime.name));
      }
    }

    return Array.from(seen).sort((a, b) =>
      runtimeLabel(a).localeCompare(runtimeLabel(b)),
    );
  }, [runtimes, workers]);

  const filteredWorkers = useMemo(() => {
    return workers.filter((worker) => {
      const matchesStatus =
        showNonActiveWorkers ||
        worker.status === "active" ||
        worker.status === "busy";
      const matchesRole =
        roleFilter === "all" || worker.worker_role === roleFilter;
      const matchesRuntime =
        runtimeFilter === "all" ||
        worker.supported_runtimes.some(
          (runtime) => normalizeRuntimeName(runtime.name) === runtimeFilter,
        );

      if (!matchesStatus || !matchesRole || !matchesRuntime) {
        return false;
      }

      return true;
    });
  }, [workers, runtimeFilter, roleFilter, showNonActiveWorkers]);

  const activeWorkers = workers.filter(
    (worker) => worker.status === "active",
  ).length;
  const busyWorkers = workers.filter(
    (worker) => worker.status === "busy",
  ).length;
  const activeTasks = workers.reduce(
    (sum, worker) => sum + worker.load.total_active,
    0,
  );

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64 rounded-xl border border-gray-200 bg-white">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded-lg">
        <p>Error: {(error as Error).message}</p>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-6">
      <div className="grid gap-4 md:grid-cols-3">
        <SummaryCard
          icon={<Bot className="w-5 h-5 text-blue-600" />}
          label="Workers"
          value={workers.length.toString()}
          tone="blue"
        />
        <SummaryCard
          icon={<Server className="w-5 h-5 text-emerald-600" />}
          label="Active / Busy"
          value={`${activeWorkers} / ${busyWorkers}`}
          tone="emerald"
        />
        <SummaryCard
          icon={<Cpu className="w-5 h-5 text-indigo-600" />}
          label="Active Tasks"
          value={activeTasks.toString()}
          tone="indigo"
        />
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-gray-200 bg-white">
        <div className="shrink-0 border-b border-gray-200 p-5">
          <div className="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
            <div>
              <h2 className="text-lg font-semibold text-gray-900">Workers</h2>
              <p className="text-sm text-gray-600 mt-1">
                Showing {filteredWorkers.length} of {totalWorkers} workers
              </p>
            </div>
            <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
              <div className="relative min-w-72">
                <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
                  <Search className="h-4 w-4 text-gray-400" />
                </div>
                <input
                  type="text"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search workers, hosts, or runtimes..."
                  className="block w-full pl-10 pr-10 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                />
                {searchQuery && (
                  <button
                    onClick={() => setSearchQuery("")}
                    className="absolute inset-y-0 right-0 pr-3 flex items-center"
                  >
                    <X className="h-4 w-4 text-gray-400 hover:text-gray-600" />
                  </button>
                )}
              </div>
              <label className="inline-flex items-center gap-2 text-sm text-gray-700">
                <input
                  type="checkbox"
                  checked={showNonActiveWorkers}
                  onChange={(e) => setShowNonActiveWorkers(e.target.checked)}
                  className="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                />
                Show inactive / error workers
              </label>
              <select
                value={roleFilter}
                onChange={(e) => setRoleFilter(e.target.value)}
                className="px-3 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
              >
                <option value="all">All roles</option>
                <option value="action">Action workers</option>
                <option value="sensor">Sensor workers</option>
              </select>
              <select
                value={runtimeFilter}
                onChange={(e) => setRuntimeFilter(e.target.value)}
                className="px-3 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
              >
                <option value="all">All runtimes</option>
                {runtimeNames.map((name) => (
                  <option key={name} value={name}>
                    {runtimeLabel(name)}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </div>

        <div
          ref={workerListRef}
          className="min-h-0 flex-1 space-y-4 overflow-y-auto p-5"
        >
          {filteredWorkers.length === 0 ? (
            <div className="rounded-lg border border-dashed border-gray-300 p-10 text-center text-gray-500">
              {workers.length === 0
                ? "No workers have registered yet."
                : "No workers match the current filters."}
            </div>
          ) : (
            filteredWorkers.map((worker) => (
              <WorkerCard key={worker.id} worker={worker} />
            ))
          )}
          <InfiniteScrollTrigger
            containerRef={workerListRef}
            fetchNextPage={fetchNextPage}
            hasNextPage={hasNextPage}
            isFetchingNextPage={isFetchingNextPage}
            itemLabel="workers"
          />
        </div>
      </div>
    </div>
  );
}

function WorkerCard({ worker }: { worker: WorkerSummary }) {
  const cordonWorker = useCordonWorker();
  const uncordonWorker = useUncordonWorker();
  const isSensorWorker = worker.worker_role === "sensor";
  const utilization = Math.max(
    0,
    Math.min(100, worker.load.utilization_percent ?? 0),
  );

  return (
    <div className="rounded-xl border border-gray-200 p-5 shadow-sm">
      <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-lg font-semibold text-gray-900">
              {worker.name}
            </h3>
            <span
              className={`inline-flex items-center px-2.5 py-0.5 rounded-full border text-xs font-medium ${getWorkerStatusClasses(worker.status)}`}
            >
              {worker.status ?? "unknown"}
            </span>
            <span className="inline-flex items-center px-2.5 py-0.5 rounded-full border border-gray-200 bg-gray-50 text-xs font-medium text-gray-700">
              {worker.worker_role}
            </span>
            <span className="inline-flex items-center px-2.5 py-0.5 rounded-full border border-gray-200 bg-gray-50 text-xs font-medium text-gray-700">
              {worker.worker_type}
            </span>
            {worker.cordoned && (
              <span className="inline-flex items-center px-2.5 py-0.5 rounded-full border border-amber-200 bg-amber-50 text-xs font-medium text-amber-800">
                cordoned
              </span>
            )}
          </div>
          <div className="mt-3 grid gap-2 text-sm text-gray-600 md:grid-cols-2 xl:grid-cols-4">
            <div>
              <span className="font-medium text-gray-900">Host:</span>{" "}
              {worker.host ?? "—"}
              {worker.port ? `:${worker.port}` : ""}
            </div>
            <div title={formatDateTime(worker.last_heartbeat)}>
              <span className="font-medium text-gray-900">Heartbeat:</span>{" "}
              {formatRelativeTime(worker.last_heartbeat)}
            </div>
            <div>
              <span className="font-medium text-gray-900">Created:</span>{" "}
              {formatDateTime(worker.created)}
            </div>
            <div>
              <span className="font-medium text-gray-900">Updated:</span>{" "}
              {formatDateTime(worker.updated)}
            </div>
          </div>
        </div>

        <div className="flex gap-2">
          {worker.cordoned ? (
            <button
              onClick={() => uncordonWorker.mutate({ id: worker.id })}
              disabled={uncordonWorker.isPending}
              className="px-3 py-1.5 rounded-md bg-emerald-600 text-white text-sm hover:bg-emerald-700 disabled:opacity-50"
            >
              Uncordon
            </button>
          ) : (
            <button
              onClick={() =>
                cordonWorker.mutate({
                  id: worker.id,
                  reason: "Cordoned from worker inventory",
                })
              }
              disabled={cordonWorker.isPending}
              className="px-3 py-1.5 rounded-md bg-amber-600 text-white text-sm hover:bg-amber-700 disabled:opacity-50"
            >
              Cordon
            </button>
          )}
        </div>

        <div className="min-w-72 rounded-lg bg-gray-50 border border-gray-200 p-4">
          <div className="flex items-center justify-between gap-4">
            <div>
              <div className="text-xs font-medium uppercase tracking-wide text-gray-500">
                {isSensorWorker ? "Sensor activity" : "Current load"}
              </div>
              <div className="mt-1 text-lg font-semibold text-gray-900">
                {isSensorWorker
                  ? `${worker.load.sensor_processes_running ?? 0} running`
                  : `${worker.load.total_active} active`}
                {(isSensorWorker
                  ? worker.load.max_concurrent_sensors
                  : worker.load.max_concurrent_executions) != null
                  ? ` / ${
                      isSensorWorker
                        ? worker.load.max_concurrent_sensors
                        : worker.load.max_concurrent_executions
                    }`
                  : ""}
              </div>
            </div>
            {worker.load.utilization_percent != null && (
              <div className="text-right">
                <div className="text-xs font-medium uppercase tracking-wide text-gray-500">
                  Utilization
                </div>
                <div className="mt-1 text-lg font-semibold text-gray-900">
                  {worker.load.utilization_percent}%
                </div>
              </div>
            )}
          </div>
          {(isSensorWorker
            ? worker.load.max_concurrent_sensors
            : worker.load.max_concurrent_executions) != null && (
            <div className="mt-3">
              <div className="h-2 rounded-full bg-gray-200 overflow-hidden">
                <div
                  className={`h-full ${getUtilizationBarClasses(worker.load.utilization_percent)}`}
                  style={{ width: `${utilization}%` }}
                />
              </div>
            </div>
          )}
          {isSensorWorker ? (
            <div className="mt-4 grid grid-cols-4 gap-2 text-center">
              <LoadMetric label="Rules" value={worker.load.active_rules ?? 0} />
              <LoadMetric
                label="Monitored"
                value={worker.load.sensor_processes_monitored ?? 0}
              />
              <LoadMetric
                label="Running"
                value={worker.load.sensor_processes_running ?? 0}
              />
              <LoadMetric
                label="Capacity"
                value={worker.load.max_concurrent_sensors ?? 0}
              />
            </div>
          ) : (
            <div className="mt-4 grid grid-cols-5 gap-2 text-center">
              <LoadMetric label="Req" value={worker.load.requested} />
              <LoadMetric label="Sched" value={worker.load.scheduling} />
              <LoadMetric label="Queued" value={worker.load.scheduled} />
              <LoadMetric label="Run" value={worker.load.running} />
              <LoadMetric label="Stop" value={worker.load.canceling} />
            </div>
          )}
          {!isSensorWorker && worker.load.queue_depth != null && (
            <div className="mt-3 text-xs text-gray-500">
              Queue depth snapshot: {worker.load.queue_depth}
            </div>
          )}
        </div>
      </div>

      <div className="mt-5">
        <div className="text-xs font-medium uppercase tracking-wide text-gray-500 mb-2">
          Supported runtimes
        </div>
        <div className="flex flex-wrap gap-2">
          {worker.supported_runtimes.length === 0 ? (
            <span className="text-sm text-gray-500">
              No runtime capabilities reported
            </span>
          ) : (
            worker.supported_runtimes.map((runtime) => (
              <RuntimeSupportChip
                key={`${worker.id}-${runtime.name}`}
                runtime={runtime}
              />
            ))
          )}
        </div>
      </div>
    </div>
  );
}

function RuntimeSupportChip({ runtime }: { runtime: WorkerRuntimeSupport }) {
  return (
    <span className="inline-flex items-center gap-2 px-3 py-1.5 rounded-full bg-blue-50 text-blue-800 text-sm">
      <span className="font-medium">{runtimeLabel(runtime.name)}</span>
      <span className="text-blue-600">
        {runtime.versions.length > 0 ? runtime.versions.join(", ") : "any"}
      </span>
    </span>
  );
}

function LoadMetric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg bg-white border border-gray-200 px-2 py-2">
      <div className="text-xs text-gray-500">{label}</div>
      <div className="text-sm font-semibold text-gray-900 mt-1">{value}</div>
    </div>
  );
}

function SummaryCard({
  icon,
  label,
  value,
  tone,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  tone: "blue" | "emerald" | "indigo";
}) {
  const toneClasses = {
    blue: "bg-blue-50 border-blue-100",
    emerald: "bg-emerald-50 border-emerald-100",
    indigo: "bg-indigo-50 border-indigo-100",
  };

  return (
    <div className={`rounded-xl border p-5 ${toneClasses[tone]}`}>
      <div className="flex items-center gap-3">
        <div className="rounded-lg bg-white/80 p-2">{icon}</div>
        <div>
          <div className="text-sm font-medium text-gray-600">{label}</div>
          <div className="text-2xl font-semibold text-gray-900">{value}</div>
        </div>
      </div>
    </div>
  );
}

function RuntimesTab({
  runtimeRef,
  canCreateRuntime,
  canUpdateRuntime,
  canDeleteRuntime,
}: {
  runtimeRef?: string;
  canCreateRuntime: boolean;
  canUpdateRuntime: boolean;
  canDeleteRuntime: boolean;
}) {
  const navigate = useNavigate();
  const [searchQuery, setSearchQuery] = useState("");
  const debouncedSearchQuery = useDebouncedValue(searchQuery.trim());
  const {
    data,
    isLoading,
    error,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
  } = useInfiniteRuntimes({ query: debouncedSearchQuery || undefined });
  const runtimes = useMemo(
    () => data?.pages.flatMap((page) => page.items) || [],
    [data?.pages],
  );
  const totalRuntimes =
    data?.pages[0]?.pagination.total_items ?? runtimes.length;
  const sidebarRef = useRef<HTMLDivElement | null>(null);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64 rounded-xl border border-gray-200 bg-white">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded-lg">
        <p>Error: {(error as Error).message}</p>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 overflow-hidden rounded-xl border border-gray-200 bg-white">
      <div
        ref={sidebarRef}
        className="w-96 border-r border-gray-200 overflow-y-auto bg-gray-50"
      >
        <div className="p-4 border-b border-gray-200 bg-white sticky top-0 z-10">
          <div className="flex items-center justify-between mb-1">
            <div>
              <h2 className="text-2xl font-bold text-gray-900">Runtimes</h2>
              <p className="text-sm text-gray-600 mt-1">
                {runtimes.length} of {totalRuntimes} runtimes
              </p>
            </div>
            {canCreateRuntime && (
              <button
                onClick={() => navigate("/runtimes/new?tab=runtimes")}
                className="inline-flex items-center px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700"
              >
                <Plus className="h-4 w-4 mr-1" />
                New Runtime
              </button>
            )}
          </div>

          <div className="mt-3 relative">
            <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
              <Search className="h-4 w-4 text-gray-400" />
            </div>
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search runtimes..."
              className="block w-full pl-10 pr-10 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery("")}
                className="absolute inset-y-0 right-0 pr-3 flex items-center"
              >
                <X className="h-4 w-4 text-gray-400 hover:text-gray-600" />
              </button>
            )}
          </div>
        </div>

        <div className="p-2 space-y-1">
          {runtimes.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">
                {debouncedSearchQuery
                  ? "No runtimes match your search"
                  : "No runtimes found"}
              </p>
            </div>
          ) : (
            runtimes.map((runtime: RuntimeSummary) => (
              <Link
                key={runtime.id}
                to={`/runtimes/${encodeURIComponent(runtime.ref)}?tab=runtimes`}
                className={`block p-3 rounded-lg transition-colors ${
                  runtimeRef === runtime.ref
                    ? "bg-blue-50 border-2 border-blue-500"
                    : "bg-white border-2 border-transparent hover:bg-gray-100 hover:border-gray-300"
                }`}
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="font-medium text-sm text-gray-900 truncate">
                    {runtime.name}
                  </div>
                  <span className="text-[11px] px-2 py-0.5 rounded-full bg-gray-100 text-gray-600">
                    {runtime.pack_ref ?? "system"}
                  </span>
                </div>
                <div className="font-mono text-xs text-gray-500 mt-1 truncate">
                  {runtime.ref}
                </div>
                {runtime.description && (
                  <div className="text-xs text-gray-400 mt-1 line-clamp-2">
                    {runtime.description}
                  </div>
                )}
              </Link>
            ))
          )}
          <InfiniteScrollTrigger
            containerRef={sidebarRef}
            fetchNextPage={fetchNextPage}
            hasNextPage={hasNextPage}
            isFetchingNextPage={isFetchingNextPage}
            itemLabel="runtimes"
          />
        </div>
      </div>

      <div className="flex-1 overflow-y-auto">
        {runtimeRef === "new" ? (
          canCreateRuntime ? (
            <RuntimeForm />
          ) : (
            <PermissionNotice message="You do not have permission to create runtimes." />
          )
        ) : runtimeRef ? (
          <RuntimeDetail
            key={runtimeRef}
            runtimeRef={runtimeRef}
            canUpdateRuntime={canUpdateRuntime}
            canDeleteRuntime={canDeleteRuntime}
          />
        ) : (
          <div className="flex items-center justify-center h-full">
            <div className="text-center text-gray-500">
              <Code2 className="mx-auto h-12 w-12 text-gray-400" />
              <h3 className="mt-2 text-sm font-medium text-gray-900">
                No runtime selected
              </h3>
              <p className="mt-1 text-sm text-gray-500">
                Select a runtime from the list to inspect or update it
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function RuntimeDetail({
  runtimeRef,
  canUpdateRuntime,
  canDeleteRuntime,
}: {
  runtimeRef: string;
  canUpdateRuntime: boolean;
  canDeleteRuntime: boolean;
}) {
  const navigate = useNavigate();
  const { data, isLoading, error } = useRuntime(runtimeRef);
  const deleteRuntime = useDeleteRuntime();
  const [isEditing, setIsEditing] = useState(false);

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error || !data?.data) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {error ? (error as Error).message : "Runtime not found"}</p>
        </div>
      </div>
    );
  }

  if (isEditing) {
    return (
      <RuntimeForm
        initialData={data.data}
        isEditing={true}
        onCancel={() => setIsEditing(false)}
      />
    );
  }

  const runtime = data.data;

  const handleDelete = async () => {
    if (!window.confirm(`Delete runtime "${runtime.ref}"?`)) {
      return;
    }

    await deleteRuntime.mutateAsync(runtime.ref);
    navigate("/runtimes?tab=runtimes");
  };

  return (
    <div className="p-6 max-w-6xl space-y-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-3">
            <h2 className="text-3xl font-bold text-gray-900">{runtime.name}</h2>
            <span className="text-xs px-2 py-1 rounded-full bg-gray-100 text-gray-700">
              {runtime.pack_ref ?? "system"}
            </span>
          </div>
          <p className="mt-2 font-mono text-sm text-gray-500">{runtime.ref}</p>
          {runtime.description && (
            <p className="mt-3 text-sm text-gray-700">{runtime.description}</p>
          )}
        </div>
        {(canUpdateRuntime || canDeleteRuntime) && (
          <div className="flex items-center gap-2">
            {canUpdateRuntime && (
              <button
                onClick={() => setIsEditing(true)}
                className="inline-flex items-center px-3 py-2 border border-gray-300 rounded-lg text-sm hover:bg-gray-50"
              >
                <Pencil className="h-4 w-4 mr-2" />
                Edit
              </button>
            )}
            {canDeleteRuntime && (
              <button
                onClick={handleDelete}
                className="inline-flex items-center px-3 py-2 border border-red-300 text-red-700 rounded-lg text-sm hover:bg-red-50"
              >
                <Trash2 className="h-4 w-4 mr-2" />
                Delete
              </button>
            )}
          </div>
        )}
      </div>

      <div className="grid gap-6 lg:grid-cols-3">
        <InfoCard label="Pack" value={runtime.pack_ref ?? "None"} />
        <InfoCard
          label="Created"
          value={new Date(runtime.created).toLocaleString()}
        />
        <InfoCard
          label="Updated"
          value={new Date(runtime.updated).toLocaleString()}
        />
      </div>

      <JsonCard title="Distributions" value={runtime.distributions} />
      <JsonCard title="Installation" value={runtime.installation} />
      <JsonCard title="Execution Config" value={runtime.execution_config} />
    </div>
  );
}

function PermissionNotice({ message }: { message: string }) {
  return (
    <div className="flex items-center justify-center h-full">
      <div className="rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
        {message}
      </div>
    </div>
  );
}

function InfoCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="bg-white rounded-lg shadow p-5">
      <div className="text-xs font-medium uppercase tracking-wide text-gray-500">
        {label}
      </div>
      <div className="mt-2 text-sm text-gray-900 break-all">{value}</div>
    </div>
  );
}

function JsonCard({ title, value }: { title: string; value: unknown }) {
  return (
    <div className="bg-white rounded-lg shadow overflow-hidden">
      <div className="px-5 py-3 border-b border-gray-200">
        <h3 className="text-sm font-semibold text-gray-900">{title}</h3>
      </div>
      <pre className="p-5 text-xs text-gray-800 overflow-x-auto bg-gray-50">
        {formatJson(value)}
      </pre>
    </div>
  );
}
