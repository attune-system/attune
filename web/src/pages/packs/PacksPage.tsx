import { Link, useParams } from "react-router-dom";
import { useInfinitePacks, usePack, useDeletePack } from "@/hooks/usePacks";
import { usePackActions } from "@/hooks/useActions";
import { usePackTriggers } from "@/hooks/useTriggers";
import { usePackSensors } from "@/hooks/useSensors";
import { usePackRules } from "@/hooks/useRules";
import { useWorkflows } from "@/hooks/useWorkflows";
import { usePackQueues } from "@/hooks/useQueues";
import { useMemo, useRef, useState } from "react";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";
import type { PackSummary, PackResponse } from "@/api";
import {
  Search,
  X,
  Package,
  Plus,
  ChevronDown,
  GitBranch,
  Settings,
} from "lucide-react";
import InfiniteScrollTrigger from "@/components/common/InfiniteScrollTrigger";
import { useDebouncedValue } from "@/hooks/useDebouncedValue";
import PackTestHistory from "@/components/packs/PackTestHistory";
import {
  usePackTestHistory,
  usePackInstallStatus,
  useExecutePackTests,
} from "@/hooks/usePackTests";
import { FlaskConical, Loader2 } from "lucide-react";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type JsonValue = any;

export default function PacksPage() {
  const { ref } = useParams<{ ref?: string }>();
  const { user } = useAuth();
  const [searchQuery, setSearchQuery] = useState("");
  const debouncedSearchQuery = useDebouncedValue(searchQuery.trim());
  const {
    data,
    isLoading,
    error,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
  } = useInfinitePacks(debouncedSearchQuery || undefined);
  const packs = useMemo(
    () => data?.pages.flatMap((page) => page.items) || [],
    [data?.pages],
  );
  const totalPacks = data?.pages[0]?.pagination.total_items ?? packs.length;
  const [showPackMenu, setShowPackMenu] = useState(false);
  const sidebarRef = useRef<HTMLDivElement | null>(null);
  const canCreatePacks = hasPermission(user, "packs", "create");
  const canInstallPacks = hasPermission(user, "packs", "install");
  const canShowPackCreateMenu = canCreatePacks || canInstallPacks;

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {(error as Error).message}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full">
      {/* Left sidebar - Packs List */}
      <div
        ref={sidebarRef}
        className="w-96 border-r border-gray-200 overflow-y-auto bg-gray-50"
      >
        <div className="p-4 border-b border-gray-200 bg-white sticky top-0 z-10">
          <div className="flex items-center justify-between mb-2">
            <h1 className="text-2xl font-bold">Packs</h1>
            {canShowPackCreateMenu && (
              <div className="relative">
                <button
                  onClick={() => setShowPackMenu(!showPackMenu)}
                  className="flex items-center gap-2 px-3 py-1 bg-blue-600 text-white rounded hover:bg-blue-700 text-sm font-medium"
                >
                  <Plus className="w-4 h-4" />
                  New Pack
                  <ChevronDown className="w-4 h-4" />
                </button>

                {showPackMenu && (
                  <>
                    <div
                      className="fixed inset-0 z-10"
                      onClick={() => setShowPackMenu(false)}
                    />
                    <div className="absolute right-0 mt-2 w-56 bg-white border border-gray-200 rounded-lg shadow-lg z-20">
                      <div className="py-1">
                        {canCreatePacks && (
                          <Link
                            to="/packs/new"
                            className="flex items-start gap-3 px-4 py-3 hover:bg-gray-50 transition-colors"
                            onClick={() => setShowPackMenu(false)}
                          >
                            <Plus className="w-5 h-5 text-blue-600 flex-shrink-0 mt-0.5" />
                            <div>
                              <div className="text-sm font-medium text-gray-900">
                                Create Empty Pack
                              </div>
                              <div className="text-xs text-gray-500 mt-0.5">
                                For ad-hoc rules, workflows, and webhooks
                              </div>
                            </div>
                          </Link>
                        )}
                        {canInstallPacks && (
                          <Link
                            to="/packs/install"
                            className={`flex items-start gap-3 px-4 py-3 hover:bg-gray-50 transition-colors ${
                              canCreatePacks ? "border-t border-gray-100" : ""
                            }`}
                            onClick={() => setShowPackMenu(false)}
                          >
                            <GitBranch className="w-5 h-5 text-purple-600 flex-shrink-0 mt-0.5" />
                            <div>
                              <div className="text-sm font-medium text-gray-900">
                                Install from Remote
                              </div>
                              <div className="text-xs text-gray-500 mt-0.5">
                                Install from git, archive, or registry
                              </div>
                            </div>
                          </Link>
                        )}
                      </div>
                    </div>
                  </>
                )}
              </div>
            )}
          </div>
          <p className="text-sm text-gray-600">
            {packs.length} of {totalPacks} packs
          </p>

          {/* Search Bar */}
          <div className="mt-3 relative">
            <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
              <Search className="h-4 w-4 text-gray-400" />
            </div>
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search packs..."
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
        <div className="p-2">
          {packs.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">
                {debouncedSearchQuery
                  ? "No packs match your search"
                  : "No packs found"}
              </p>
              {debouncedSearchQuery ? (
                <button
                  onClick={() => setSearchQuery("")}
                  className="mt-2 text-sm text-blue-600 hover:text-blue-800"
                >
                  Clear search
                </button>
              ) : canShowPackCreateMenu ? (
                <div className="mt-3 flex flex-col gap-2 items-center">
                  {canCreatePacks && (
                    <Link
                      to="/packs/new"
                      className="text-sm text-blue-600 hover:text-blue-800"
                    >
                      Create an empty pack
                    </Link>
                  )}
                  {canCreatePacks && canInstallPacks && (
                    <span className="text-xs text-gray-400">or</span>
                  )}
                  {canInstallPacks && (
                    <Link
                      to="/packs/install"
                      className="text-sm text-blue-600 hover:text-blue-800"
                    >
                      Install from remote
                    </Link>
                  )}
                </div>
              ) : null}
            </div>
          ) : (
            <div className="space-y-1">
              {packs.map((pack: PackSummary) => (
                <Link
                  key={pack.id}
                  to={`/packs/${pack.ref}`}
                  className={`block p-3 rounded-lg transition-colors ${
                    ref === pack.ref
                      ? "bg-blue-50 border-2 border-blue-500"
                      : "bg-white border-2 border-transparent hover:bg-gray-100 hover:border-gray-300"
                  }`}
                >
                  <div className="flex items-center justify-between">
                    <div className="font-medium text-sm text-gray-900 truncate">
                      {pack.label}
                    </div>
                    {pack.is_standard && (
                      <span className="ml-2 inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800">
                        Standard
                      </span>
                    )}
                  </div>
                  <div className="font-mono text-xs text-gray-500 mt-1 truncate">
                    {pack.ref}
                  </div>
                  <div className="text-xs text-gray-400 mt-1 truncate">
                    v{pack.version}
                  </div>
                  {pack.description && (
                    <div className="text-xs text-gray-400 mt-1 line-clamp-2">
                      {pack.description}
                    </div>
                  )}
                </Link>
              ))}
            </div>
          )}
          <InfiniteScrollTrigger
            containerRef={sidebarRef}
            fetchNextPage={fetchNextPage}
            hasNextPage={hasNextPage}
            isFetchingNextPage={isFetchingNextPage}
            itemLabel="packs"
          />
        </div>
      </div>

      {/* Right panel - Pack Detail or Empty State */}
      <div className="flex-1 overflow-y-auto">
        {ref ? (
          <PackDetail packRef={ref} />
        ) : (
          <div className="flex items-center justify-center h-full">
            <div className="text-center text-gray-500">
              <Package className="mx-auto h-12 w-12 text-gray-400" />
              <h3 className="mt-2 text-sm font-medium text-gray-900">
                No pack selected
              </h3>
              <p className="mt-1 text-sm text-gray-500">
                Select a pack from the list to view its details
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function PackDetail({ packRef }: { packRef: string }) {
  const { user } = useAuth();
  const { data: pack, isLoading, error } = usePack(packRef);
  const { data: actions } = usePackActions(packRef);
  const { data: triggers } = usePackTriggers(packRef);
  const { data: sensors } = usePackSensors(packRef);
  const { data: rules } = usePackRules(packRef);
  const { data: workflows } = useWorkflows({ packRef, pageSize: 100 });
  const { data: queues } = usePackQueues(packRef);
  const { data: packTests, isLoading: packTestsLoading } = usePackTestHistory(
    packRef,
    { pageSize: 10 },
  );
  const runTests = useExecutePackTests();
  const packInstallStatus = usePackInstallStatus(packRef, true);
  const deletePack = useDeletePack();
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [expandedComponent, setExpandedComponent] = useState<string | null>(
    null,
  );
  const canConfigurePacks = hasPermission(user, "packs", "configure");
  const canDeletePacks = hasPermission(user, "packs", "delete");

  const handleDelete = async () => {
    try {
      await deletePack.mutateAsync(packRef);
      window.location.href = "/packs";
    } catch (err) {
      console.error("Failed to delete pack:", err);
    }
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  if (error || !pack) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {error ? (error as Error).message : "Pack not found"}</p>
        </div>
      </div>
    );
  }

  const packActions =
    actions?.pagination.total_items ?? actions?.items.length ?? 0;
  const packTriggers =
    triggers?.pagination.total_items ?? triggers?.items.length ?? 0;
  const packSensors =
    sensors?.pagination.total_items ?? sensors?.items.length ?? 0;
  const packRules = rules?.pagination.total_items ?? rules?.items.length ?? 0;
  const packWorkflows =
    workflows?.pagination.total_items ?? workflows?.items.length ?? 0;
  const packQueues =
    queues?.pagination.total_items ?? queues?.items.length ?? 0;
  const componentGroups = [
    {
      label: "Actions",
      count: packActions,
      listTo: `/actions?pack=${encodeURIComponent(packRef)}`,
      items: actions?.items ?? [],
      itemTo: (componentRef: string) =>
        `/actions/${encodeURIComponent(componentRef)}`,
    },
    {
      label: "Triggers",
      count: packTriggers,
      listTo: `/triggers?pack=${encodeURIComponent(packRef)}`,
      items: triggers?.items ?? [],
      itemTo: (componentRef: string) =>
        `/triggers/${encodeURIComponent(componentRef)}`,
    },
    {
      label: "Sensors",
      count: packSensors,
      listTo: `/sensors?pack=${encodeURIComponent(packRef)}`,
      items: sensors?.items ?? [],
      itemTo: (componentRef: string) =>
        `/sensors/${encodeURIComponent(componentRef)}`,
    },
    {
      label: "Rules",
      count: packRules,
      listTo: `/rules?pack=${encodeURIComponent(packRef)}`,
      items: rules?.items ?? [],
      itemTo: (componentRef: string) =>
        `/rules/${encodeURIComponent(componentRef)}`,
    },
    {
      label: "Workflows",
      count: packWorkflows,
      listTo: `/actions?pack=${encodeURIComponent(packRef)}`,
      items: workflows?.items ?? [],
      itemTo: (componentRef: string) =>
        `/actions/${encodeURIComponent(componentRef)}`,
    },
    {
      label: "Queues",
      count: packQueues,
      listTo: `/queues?pack=${encodeURIComponent(packRef)}`,
      items: queues?.items ?? [],
      itemTo: (componentRef: string) =>
        `/queues/${encodeURIComponent(componentRef)}`,
    },
  ];

  return (
    <div className="p-6 max-w-7xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <h1 className="text-3xl font-bold">{pack.data?.label}</h1>
            {pack.data?.is_standard && (
              <span className="inline-flex items-center px-3 py-1 rounded-full text-sm font-medium bg-blue-100 text-blue-800">
                Standard
              </span>
            )}
          </div>
          <div className="flex gap-2">
            {canConfigurePacks && (
              <Link
                to={`/packs/${packRef}/edit`}
                className="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700"
              >
                Edit
              </Link>
            )}
            {canDeletePacks && (
              <button
                onClick={() => setShowDeleteConfirm(true)}
                disabled={deletePack.isPending}
                className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700 disabled:opacity-50"
              >
                Delete
              </button>
            )}
          </div>
        </div>
      </div>

      {/* Delete Confirmation Modal */}
      {showDeleteConfirm && canDeletePacks && (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
          <div className="bg-white rounded-lg p-6 max-w-md">
            <h3 className="text-xl font-bold mb-4">Confirm Delete</h3>
            <p className="mb-6">
              Are you sure you want to delete pack{" "}
              <strong>{pack.data?.label}</strong>? This will also delete all
              associated actions, triggers, sensors, and rules.
            </p>
            <div className="flex justify-end gap-3">
              <button
                onClick={() => setShowDeleteConfirm(false)}
                className="px-4 py-2 bg-gray-200 rounded hover:bg-gray-300"
              >
                Cancel
              </button>
              <button
                onClick={handleDelete}
                className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Main Info Card */}
        <div className="lg:col-span-2 space-y-6">
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-xl font-semibold mb-4">Pack Information</h2>
            <dl className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div>
                <dt className="text-sm font-medium text-gray-500">Reference</dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {pack.data?.ref}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Label</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {pack.data?.label}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Version</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {pack.data?.version}
                </dd>
              </div>

              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-gray-500">
                  Description
                </dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {pack.data?.description || "No description provided"}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Created</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(pack.data?.created || "").toLocaleString()}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Updated</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(pack.data?.updated || "").toLocaleString()}
                </dd>
              </div>
            </dl>
          </div>

          {/* Pack Config */}
          <PackConfiguration pack={pack.data} />

          {/* Pack Tests */}
          <div className="bg-white shadow rounded-lg p-6">
            <div className="flex items-center justify-between mb-4">
              <div className="flex items-center gap-2">
                <FlaskConical className="w-5 h-5 text-gray-600" />
                <h2 className="text-xl font-semibold">Pack Tests</h2>
              </div>
              <button
                type="button"
                onClick={() => runTests.mutate(packRef)}
                disabled={runTests.isPending}
                className="inline-flex items-center gap-2 px-3 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700 text-sm font-medium disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {runTests.isPending ? (
                  <Loader2 className="w-4 h-4 animate-spin" />
                ) : (
                  <FlaskConical className="w-4 h-4" />
                )}
                Run Tests
              </button>
            </div>

            {packInstallStatus.data?.data &&
              (packInstallStatus.data.data.status === "pending" ||
                packInstallStatus.data.data.status === "running") && (
                <div className="mb-4 flex items-center gap-2 rounded-lg bg-blue-50 border border-blue-200 px-4 py-3 text-sm text-blue-800">
                  <Loader2 className="w-4 h-4 animate-spin" />
                  Pack tests are running on a worker...
                </div>
              )}
            {packInstallStatus.data?.data?.errorMessage &&
              packInstallStatus.data.data.status === "failed" && (
                <div className="mb-4 rounded-lg bg-red-50 border border-red-200 px-4 py-3 text-sm text-red-700">
                  {packInstallStatus.data.data.errorMessage}
                </div>
              )}

            <PackTestHistory
              executions={packTests?.items ?? []}
              isLoading={packTestsLoading}
            />
          </div>
        </div>

        {/* Sidebar */}
        <div className="space-y-6">
          {/* Components */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Components</h2>
            <div className="space-y-3">
              {componentGroups.map((component) => {
                const isExpanded = expandedComponent === component.label;
                const contentId = `pack-${packRef}-${component.label.toLowerCase()}`;

                return (
                  <div
                    key={component.label}
                    className="overflow-hidden rounded border border-gray-200"
                  >
                    <div className="flex items-center justify-between px-3 py-2 text-sm">
                      <Link
                        to={component.listTo}
                        className="font-medium text-gray-700 hover:text-blue-600 hover:underline"
                      >
                        {component.label}
                      </Link>
                      <button
                        type="button"
                        onClick={() =>
                          setExpandedComponent((current) =>
                            current === component.label
                              ? null
                              : component.label,
                          )
                        }
                        aria-expanded={isExpanded}
                        aria-controls={contentId}
                        aria-label={`${isExpanded ? "Collapse" : "Expand"} ${component.label}`}
                        className="flex items-center gap-2 rounded px-1 py-0.5 hover:bg-gray-100"
                      >
                        <span className="rounded bg-gray-100 px-2 py-0.5 text-sm font-semibold text-gray-900">
                          {component.count}
                        </span>
                        <ChevronDown
                          className={`h-4 w-4 text-gray-500 transition-transform ${
                            isExpanded ? "rotate-180" : ""
                          }`}
                        />
                      </button>
                    </div>

                    {isExpanded && (
                      <div
                        id={contentId}
                        className="border-t border-gray-200 bg-gray-50 p-2"
                      >
                        {component.items.length === 0 ? (
                          <p className="px-2 py-1 text-sm text-gray-500">
                            No {component.label.toLowerCase()} in this pack.
                          </p>
                        ) : (
                          <ul className="space-y-1">
                            {component.items.map((item) => (
                              <li key={item.ref}>
                                <Link
                                  to={component.itemTo(item.ref)}
                                  className="block rounded px-2 py-2 text-sm transition-colors hover:bg-white"
                                >
                                  <span className="block font-medium text-blue-600">
                                    {item.label}
                                  </span>
                                  <span className="block truncate font-mono text-xs text-gray-500">
                                    {item.ref}
                                  </span>
                                  {item.description && (
                                    <span className="mt-1 block line-clamp-2 text-xs text-gray-600">
                                      {item.description}
                                    </span>
                                  )}
                                </Link>
                              </li>
                            ))}
                          </ul>
                        )}
                        {component.count > component.items.length && (
                          <p className="px-2 pt-2 text-xs text-gray-500">
                            Showing the first {component.items.length} of{" "}
                            {component.count} {component.label.toLowerCase()}.
                          </p>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>

          {/* Dependencies */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Dependencies</h2>
            <div className="space-y-4">
              <div>
                <dt className="text-sm font-medium text-gray-500 mb-2">
                  Runtime Dependencies
                </dt>
                {pack.data?.runtime_deps &&
                pack.data.runtime_deps.length > 0 ? (
                  <div className="flex flex-wrap gap-2">
                    {pack.data.runtime_deps.map((dep) => (
                      <span
                        key={dep}
                        className="inline-flex items-center rounded bg-blue-50 px-2 py-0.5 text-xs font-mono text-blue-700 border border-blue-200"
                      >
                        {dep}
                      </span>
                    ))}
                  </div>
                ) : (
                  <p className="text-sm text-gray-400 italic">None declared</p>
                )}
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500 mb-2">
                  Pack Dependencies
                </dt>
                {pack.data?.dependencies &&
                pack.data.dependencies.length > 0 ? (
                  <div className="flex flex-wrap gap-2">
                    {pack.data.dependencies.map((dep) => (
                      <span
                        key={dep}
                        className="inline-flex items-center rounded bg-purple-50 px-2 py-0.5 text-xs font-mono text-purple-700 border border-purple-200"
                      >
                        {dep}
                      </span>
                    ))}
                  </div>
                ) : (
                  <p className="text-sm text-gray-400 italic">None declared</p>
                )}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// Helper component to display pack configuration
function PackConfiguration({ pack }: { pack: PackResponse | undefined }) {
  if (!pack) return null;

  const confSchema = pack.conf_schema || {};
  const config = pack.config || {};
  const properties =
    confSchema && typeof confSchema === "object" && !Array.isArray(confSchema)
      ? confSchema.properties || {}
      : {};
  const configEntries =
    config && typeof config === "object" && !Array.isArray(config)
      ? Object.entries(config)
      : [];
  const entryKeys = Array.from(
    new Set([...Object.keys(properties), ...configEntries.map(([key]) => key)]),
  ).sort((left, right) => left.localeCompare(right));

  return (
    <div className="bg-white shadow rounded-lg p-6">
      <div className="flex items-center gap-2 mb-4">
        <Settings className="w-5 h-5 text-gray-600" />
        <h2 className="text-xl font-semibold">Pack Config</h2>
      </div>
      {entryKeys.length === 0 ? (
        <div className="rounded-lg border border-dashed border-gray-300 bg-gray-50 px-4 py-6 text-sm text-gray-500">
          No pack configuration is currently set.
        </div>
      ) : (
        <div className="space-y-4">
          {entryKeys.map((key) => {
            const schema = properties[key] as JsonValue;
            const value = config[key];
            const hasValue = value !== undefined && value !== null;
            const displayValue = hasValue ? value : schema?.default;
            const isUsingDefault = !hasValue && schema?.default !== undefined;

            return (
              <div
                key={key}
                className="border-b border-gray-200 pb-4 last:border-0 last:pb-0"
              >
                <div className="flex items-start justify-between">
                  <div className="flex-1">
                    <div className="flex items-center gap-2">
                      <dt className="text-sm font-medium text-gray-900 font-mono">
                        {key}
                      </dt>
                      {schema?.type && (
                        <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-gray-100 text-gray-700">
                          {schema.type}
                        </span>
                      )}
                      {isUsingDefault && (
                        <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-yellow-100 text-yellow-800">
                          default
                        </span>
                      )}
                    </div>
                    {schema?.description && (
                      <p className="mt-1 text-sm text-gray-600">
                        {schema.description}
                      </p>
                    )}
                  </div>
                  <dd className="ml-4 text-sm text-right">
                    <ConfigValue value={displayValue} type={schema?.type} />
                  </dd>
                </div>
                {schema?.minimum !== undefined &&
                  schema?.maximum !== undefined && (
                    <p className="mt-1 text-xs text-gray-500">
                      Range: {schema.minimum} - {schema.maximum}
                    </p>
                  )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// Helper component to render config values based on type
function ConfigValue({ value, type }: { value: JsonValue; type?: string }) {
  if (value === undefined || value === null) {
    return <span className="text-gray-400 italic">not set</span>;
  }

  switch (type) {
    case "boolean":
      return (
        <span
          className={`inline-flex items-center px-2 py-1 rounded-full text-xs font-medium ${
            value ? "bg-green-100 text-green-800" : "bg-gray-100 text-gray-800"
          }`}
        >
          {value ? "✓ true" : "✗ false"}
        </span>
      );
    case "integer":
    case "number":
      return <span className="font-mono text-gray-900">{value}</span>;
    case "string":
      if (typeof value === "string" && value.length > 50) {
        return (
          <span className="text-gray-900 text-xs break-all max-w-xs block">
            {value}
          </span>
        );
      }
      return <span className="text-gray-900">{String(value)}</span>;
    case "array":
      if (Array.isArray(value)) {
        return (
          <span className="text-gray-900 text-xs">[{value.length} items]</span>
        );
      }
      return <span className="text-gray-900">{JSON.stringify(value)}</span>;
    case "object":
      if (typeof value === "object") {
        return (
          <span className="text-gray-900 text-xs">
            {"{" + Object.keys(value).length + " keys}"}
          </span>
        );
      }
      return <span className="text-gray-900">{JSON.stringify(value)}</span>;
    default:
      // For unknown types, try to display intelligently
      if (typeof value === "boolean") {
        return (
          <span
            className={`inline-flex items-center px-2 py-1 rounded-full text-xs font-medium ${
              value
                ? "bg-green-100 text-green-800"
                : "bg-gray-100 text-gray-800"
            }`}
          >
            {value ? "✓ true" : "✗ false"}
          </span>
        );
      }
      if (typeof value === "object") {
        return (
          <pre className="text-xs text-gray-900 max-w-xs overflow-auto">
            {JSON.stringify(value, null, 2)}
          </pre>
        );
      }
      return <span className="text-gray-900">{String(value)}</span>;
  }
}
