import { Link, useParams, useNavigate, useSearchParams } from "react-router-dom";
import {
  useActions,
  useAction,
  useDeleteAction,
  useUpdateAction,
} from "@/hooks/useActions";
import { useExecutions } from "@/hooks/useExecutions";
import { usePermissionSets } from "@/hooks/usePermissions";
import { useEffect, useMemo, useRef, useState } from "react";
import type {
  ActionResponse,
  ActionSummary,
  ExecutionSummary,
  PermissionSetSummary,
  UpdateActionRequest,
  WorkerToleration,
  WorkerAffinity,
} from "@/api";
import {
  ActionReferenceVisibility,
  LogRetentionLimitPatch,
  LogRetentionPolicyPatch,
  RetentionPolicyType,
  TimeoutSecondsPatch,
} from "@/api";
import type { ParamSchemaProperty } from "@/components/common/ParamSchemaForm";
import {
  ChevronDown,
  ChevronRight,
  Search,
  X,
  Play,
  Plus,
  GitBranch,
  Pencil,
  Settings,
} from "lucide-react";
import ErrorDisplay from "@/components/common/ErrorDisplay";
import ExecuteActionModal from "@/components/common/ExecuteActionModal";
import MultiSelect from "@/components/common/MultiSelect";
import OnOffSwitch from "@/components/common/OnOffSwitch";
import RetentionPolicyControls from "@/components/common/RetentionPolicyControls";
import {
  formatRetention,
  type RetentionPolicy,
} from "@/components/common/retentionPolicy";
import {
  WorkerSelectorEditor,
  WorkerTolerationsEditor,
  WorkerAffinityEditor,
} from "@/components/common/WorkerPlacementEditors";
import { extractProperties } from "@/components/common/ParamSchemaForm";
import { STANDARD_EXECUTION_ACCESS_REF } from "@/lib/permissions";

export default function ActionsPage() {
  const { ref } = useParams<{ ref?: string }>();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { data, isLoading, error } = useActions();
  const actions = useMemo(() => data?.items || [], [data?.items]);
  const [collapsedPacks, setCollapsedPacks] = useState<Set<string>>(new Set());
  const [searchQuery, setSearchQuery] = useState("");
  const sidebarRef = useRef<HTMLDivElement | null>(null);
  const headerRef = useRef<HTMLDivElement | null>(null);
  const packSectionRefs = useRef<Record<string, HTMLDivElement | null>>({});

  // Filter actions based on search query
  const filteredActions = useMemo(() => {
    if (!searchQuery.trim()) return actions;
    const query = searchQuery.toLowerCase();
    return actions.filter((action: ActionSummary) => {
      return (
        action.label?.toLowerCase().includes(query) ||
        action.ref?.toLowerCase().includes(query) ||
        action.description?.toLowerCase().includes(query) ||
        action.pack_ref?.toLowerCase().includes(query)
      );
    });
  }, [actions, searchQuery]);

  // Group filtered actions by pack
  const actionsByPack = useMemo(() => {
    const grouped = new Map<string, ActionSummary[]>();
    filteredActions.forEach((action: ActionSummary) => {
      const packRef = action.pack_ref;
      if (!grouped.has(packRef)) {
        grouped.set(packRef, []);
      }
      grouped.get(packRef)!.push(action);
    });
    // Sort packs alphabetically
    return new Map(
      [...grouped.entries()].sort((a, b) => a[0].localeCompare(b[0])),
    );
  }, [filteredActions]);

  const requestedPack = searchParams.get("pack")?.trim() || "";
  const focusedPack = useMemo(() => {
    if (!requestedPack) {
      return null;
    }

    return actionsByPack.has(requestedPack) ? requestedPack : null;
  }, [actionsByPack, requestedPack]);

  const orderedPackEntries = useMemo(() => {
    const entries = Array.from(actionsByPack.entries());
    if (!focusedPack) {
      return entries;
    }

    return entries.sort(([left], [right]) => {
      if (left === focusedPack) {
        return -1;
      }
      if (right === focusedPack) {
        return 1;
      }
      return left.localeCompare(right);
    });
  }, [actionsByPack, focusedPack]);

  useEffect(() => {
    if (!focusedPack) {
      return;
    }

    const target = packSectionRefs.current[focusedPack];
    const container = sidebarRef.current;
    if (!target || !container) {
      return;
    }

    const stickyHeaderHeight = headerRef.current?.offsetHeight ?? 0;
    const targetTop =
      target.offsetTop - stickyHeaderHeight - 8;

    container.scrollTo({
      top: Math.max(0, targetTop),
      behavior: "auto",
    });
  }, [focusedPack, orderedPackEntries.length]);

  const togglePack = (packRef: string) => {
    setCollapsedPacks((prev) => {
      const next = new Set(prev);
      if (next.has(packRef)) {
        next.delete(packRef);
      } else {
        next.add(packRef);
      }
      return next;
    });
  };

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
        <ErrorDisplay error={error} title="Failed to load actions" />
      </div>
    );
  }

  return (
    <div className="flex h-full">
      {/* Left sidebar - Actions List */}
      <div ref={sidebarRef} className="w-96 border-r border-gray-200 overflow-y-auto bg-gray-50">
        <div ref={headerRef} className="p-4 border-b border-gray-200 bg-white sticky top-0 z-10">
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-2xl font-bold">Actions</h1>
              <p className="text-sm text-gray-600 mt-1">
                {filteredActions.length} of {actions.length} actions
                {focusedPack ? ` • Focused pack: ${focusedPack}` : ""}
              </p>
            </div>
            <button
              onClick={() => navigate("/actions/workflows/new")}
              className="flex items-center gap-1.5 px-3 py-2 bg-blue-600 text-white text-sm font-medium rounded-lg hover:bg-blue-700 transition-colors shadow-sm"
              title="Create a new workflow action"
            >
              <Plus className="w-4 h-4" />
              Workflow
            </button>
          </div>

          {/* Search Bar */}
          <div className="mt-3 relative">
            <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
              <Search className="h-4 w-4 text-gray-400" />
            </div>
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search actions..."
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
          {actions.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No actions found</p>
            </div>
          ) : filteredActions.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No actions match your search</p>
              <button
                onClick={() => setSearchQuery("")}
                className="mt-2 text-sm text-blue-600 hover:text-blue-800"
              >
                Clear search
              </button>
            </div>
          ) : (
            <div className="space-y-2">
              {orderedPackEntries.map(
                ([packRef, packActions]) => {
                  const isCollapsed =
                    focusedPack !== null && packRef !== focusedPack
                      ? true
                      : collapsedPacks.has(packRef);
                  return (
                    <div
                      key={packRef}
                      ref={(element) => {
                        packSectionRefs.current[packRef] = element;
                      }}
                      className="bg-white rounded-lg shadow-sm overflow-hidden"
                    >
                      {/* Pack Header */}
                      <button
                        onClick={() => togglePack(packRef)}
                        className="w-full px-3 py-2 flex items-center justify-between hover:bg-gray-50 transition-colors border-b border-gray-200"
                      >
                        <div className="flex items-center gap-2">
                          {isCollapsed ? (
                            <ChevronRight className="w-4 h-4 text-gray-500" />
                          ) : (
                            <ChevronDown className="w-4 h-4 text-gray-500" />
                          )}
                          <span className="font-semibold text-sm text-gray-900">
                            {packRef}
                          </span>
                        </div>
                        <span className="text-xs text-gray-500 bg-gray-100 px-2 py-0.5 rounded">
                          {packActions.length}
                        </span>
                      </button>

                      {/* Actions List */}
                      {!isCollapsed && (
                        <div className="p-1">
                          {packActions.map((action: ActionSummary) => (
                            <Link
                              key={action.id}
                              to={`/actions/${action.ref}`}
                              className={`block p-3 rounded transition-colors ${
                                ref === action.ref
                                  ? "bg-blue-50 border-2 border-blue-500"
                                  : "border-2 border-transparent hover:bg-gray-50"
                              }`}
                            >
                              <div className="flex items-center justify-between gap-2">
                                <div className="font-medium text-sm text-gray-900 truncate flex items-center gap-1.5">
                                  {action.workflow_def && (
                                    <span title="Workflow">
                                      <GitBranch className="w-3.5 h-3.5 text-purple-500 flex-shrink-0" />
                                    </span>
                                  )}
                                  {action.label}
                                </div>
                                <span
                                  className={`ml-2 inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium ${
                                    action.enabled
                                      ? "bg-green-100 text-green-800"
                                      : "bg-gray-100 text-gray-800"
                                  }`}
                                >
                                  {action.enabled ? "Enabled" : "Disabled"}
                                </span>
                              </div>
                              <div className="font-mono text-xs text-gray-500 mt-1 truncate">
                                {action.ref}
                              </div>
                              {action.description && (
                                <div className="text-xs text-gray-400 mt-1 line-clamp-2">
                                  {action.description}
                                </div>
                              )}
                            </Link>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                },
              )}
            </div>
          )}
        </div>
      </div>

      {/* Right panel - Action Detail or Empty State */}
      <div className="flex-1 overflow-y-auto">
        {ref ? (
          <ActionDetail actionRef={ref} />
        ) : (
          <div className="flex items-center justify-center h-full">
            <div className="text-center text-gray-500">
              <svg
                className="mx-auto h-12 w-12 text-gray-400"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth={2}
                  d="M13 10V3L4 14h7v7l9-11h-7z"
                />
              </svg>
              <h3 className="mt-2 text-sm font-medium text-gray-900">
                No action selected
              </h3>
              <p className="mt-1 text-sm text-gray-500">
                Select an action from the list to view its details
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function ActionDetail({ actionRef }: { actionRef: string }) {
  const navigate = useNavigate();
  const { data: action, isLoading, error } = useAction(actionRef);
  const { data: executionsData } = useExecutions({
    actionRef: actionRef,
    pageSize: 10,
  });
  const deleteAction = useDeleteAction();
  const updateAction = useUpdateAction();
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [showExecuteModal, setShowExecuteModal] = useState(false);
  const [showConfigureModal, setShowConfigureModal] = useState(false);

  const handleDelete = async () => {
    try {
      await deleteAction.mutateAsync(actionRef);
      // Navigate back to actions list without selection
      window.location.href = "/actions";
    } catch (err) {
      console.error("Failed to delete action:", err);
    }
  };

  const handleToggleEnabled = async (enabled: boolean) => {
    try {
      await updateAction.mutateAsync({
        ref: actionRef,
        data: { enabled },
      });
    } catch (err) {
      console.error("Failed to toggle action enabled status:", err);
    }
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  if (error || !action) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {error ? (error as Error).message : "Action not found"}</p>
        </div>
      </div>
    );
  }

  const executions = executionsData?.items || [];
  const actionDetails = action.data as ActionResponse;
  const paramSchema = action.data?.param_schema || {};
  const properties = extractProperties(paramSchema);
  const paramEntries = Object.entries(properties);
  const outSchema = action.data?.out_schema || {};
  const outProperties = extractProperties(outSchema);
  const outEntries = Object.entries(outProperties);

  return (
    <div className="p-6 max-w-7xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <h1 className="text-3xl font-bold">
              <span className="text-gray-500">{action.data?.pack_ref}.</span>
              {action.data?.label}
            </h1>
            <div className="inline-flex items-center">
              <OnOffSwitch
                checked={actionDetails.enabled}
                disabled={updateAction.isPending}
                ariaLabel="Action enabled"
                onChange={(checked) => {
                  void handleToggleEnabled(checked);
                }}
              />
              <span className="ms-3 text-sm font-medium">
                {updateAction.isPending ? (
                  <span className="text-gray-400">Updating...</span>
                ) : (
                  <span
                    className={
                      actionDetails.enabled ? "text-green-700" : "text-gray-700"
                    }
                  >
                    {actionDetails.enabled ? "Enabled" : "Disabled"}
                  </span>
                )}
              </span>
            </div>
          </div>
          <div className="flex gap-2">
            {action.data?.workflow_def && (
              <button
                onClick={() =>
                  navigate(`/actions/workflows/${action.data!.ref}/edit`)
                }
                className="px-4 py-2 bg-purple-600 text-white rounded hover:bg-purple-700 flex items-center gap-2"
              >
                <Pencil className="h-4 w-4" />
                Edit Workflow
              </button>
            )}
            <button
              onClick={() => setShowExecuteModal(true)}
              disabled={!actionDetails.enabled}
              title={
                actionDetails.enabled
                  ? "Execute action"
                  : "Disabled actions cannot be executed"
              }
              className="px-4 py-2 bg-green-600 text-white rounded hover:bg-green-700 flex items-center gap-2 disabled:cursor-not-allowed disabled:bg-gray-300 disabled:text-gray-500"
            >
              <Play className="h-4 w-4" />
              Execute
            </button>
            <button
              onClick={() => setShowConfigureModal(true)}
              className="px-4 py-2 bg-gray-600 text-white rounded hover:bg-gray-700 flex items-center gap-2"
            >
              <Settings className="h-4 w-4" />
              Configure
            </button>
            {/* Only show delete button for ad-hoc actions (not from pack installation) */}
            {action.data?.is_adhoc && (
              <button
                onClick={() => setShowDeleteConfirm(true)}
                disabled={deleteAction.isPending}
                className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700 disabled:opacity-50"
              >
                Delete
              </button>
            )}
          </div>
        </div>
      </div>

      {/* Delete Confirmation Modal */}
      {showDeleteConfirm && (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
          <div className="bg-white rounded-lg p-6 max-w-md">
            <h3 className="text-xl font-bold mb-4">Confirm Delete</h3>
            <p className="mb-6">
              Are you sure you want to delete action{" "}
              <strong>
                {action.data?.pack_ref}.{action.data?.label}
              </strong>
              ? This will also delete all associated executions.
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

      {/* Execute Action Modal */}
      {showExecuteModal && (
        <ExecuteActionModal
          action={action.data!}
          onClose={() => setShowExecuteModal(false)}
        />
      )}

      {/* Configure Action Modal */}
      {showConfigureModal && (
        <ConfigureActionModal
          action={action.data!}
          onClose={() => setShowConfigureModal(false)}
        />
      )}

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Main Info Card */}
        <div className="lg:col-span-2 space-y-6">
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-xl font-semibold mb-4">Action Information</h2>
            <dl className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div>
                <dt className="text-sm font-medium text-gray-500">Reference</dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {action.data?.ref}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Label</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {action.data?.label}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Pack</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  <Link
                    to={`/packs/${action.data?.pack_ref}`}
                    className="text-blue-600 hover:text-blue-800"
                  >
                    {action.data?.pack_ref}
                  </Link>
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">
                  Entry Point
                </dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {action.data?.entrypoint}
                </dd>
              </div>
              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-gray-500">
                  Description
                </dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {action.data?.description || "No description provided"}
                </dd>
              </div>
              {action.data?.runtime && (
                <div>
                  <dt className="text-sm font-medium text-gray-500">Runtime</dt>
                  <dd className="mt-1 text-sm text-gray-900 font-mono">
                    {action.data.runtime_ref || `#${action.data.runtime}`}
                    {action.data.runtime_version_constraint && (
                      <span className="ml-1 text-gray-500">
                        {action.data.runtime_version_constraint}
                      </span>
                    )}
                  </dd>
                </div>
              )}
              <div>
                <dt className="text-sm font-medium text-gray-500">Created</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(action.data?.created || "").toLocaleString()}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Updated</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(action.data?.updated || "").toLocaleString()}
                </dd>
              </div>
            </dl>

            {paramEntries.length > 0 && (
              <div className="mt-6">
                <h3 className="text-sm font-medium text-gray-900 mb-3">
                  Parameters
                </h3>
                <div className="space-y-3">
                  {paramEntries.map(
                    ([key, param]: [string, ParamSchemaProperty]) => (
                      <div
                        key={key}
                        className="border border-gray-200 rounded p-3"
                      >
                        <div className="flex items-start justify-between">
                          <div className="flex-1">
                            <div className="flex items-center gap-2">
                              <span className="font-mono font-semibold text-sm">
                                {key}
                              </span>
                              {param?.required && (
                                <span className="text-xs px-2 py-0.5 bg-red-100 text-red-700 rounded">
                                  Required
                                </span>
                              )}
                              {param?.secret && (
                                <span className="text-xs px-2 py-0.5 bg-yellow-100 text-yellow-700 rounded">
                                  Secret
                                </span>
                              )}
                              <span className="text-xs px-2 py-0.5 bg-gray-100 text-gray-700 rounded">
                                {param?.type || "any"}
                              </span>
                            </div>
                            {param?.description && (
                              <p className="text-sm text-gray-600 mt-1">
                                {param.description}
                              </p>
                            )}
                            {param?.default !== undefined && (
                              <p className="text-xs text-gray-500 mt-1">
                                Default:{" "}
                                <code className="bg-gray-100 px-1 rounded">
                                  {JSON.stringify(param.default)}
                                </code>
                              </p>
                            )}
                            {param?.enum && param.enum.length > 0 && (
                              <p className="text-xs text-gray-500 mt-1">
                                Values:{" "}
                                {param.enum
                                  .map((v: string) => `"${v}"`)
                                  .join(", ")}
                              </p>
                            )}
                          </div>
                        </div>
                      </div>
                    ),
                  )}
                </div>
              </div>
            )}

            {outEntries.length > 0 && (
              <div className="mt-6">
                <h3 className="text-sm font-medium text-gray-900 mb-3">
                  Output Schema
                </h3>
                <div className="space-y-3">
                  {outEntries.map(
                    ([key, param]: [string, ParamSchemaProperty]) => (
                      <div
                        key={key}
                        className="border border-gray-200 rounded p-3"
                      >
                        <div className="flex items-start justify-between">
                          <div className="flex-1">
                            <div className="flex items-center gap-2">
                              <span className="font-mono font-semibold text-sm">
                                {key}
                              </span>
                              {param?.required && (
                                <span className="text-xs px-2 py-0.5 bg-red-100 text-red-700 rounded">
                                  Required
                                </span>
                              )}
                              {param?.secret && (
                                <span className="text-xs px-2 py-0.5 bg-yellow-100 text-yellow-700 rounded">
                                  Secret
                                </span>
                              )}
                              <span className="text-xs px-2 py-0.5 bg-gray-100 text-gray-700 rounded">
                                {param?.type || "any"}
                              </span>
                            </div>
                            {param?.description && (
                              <p className="text-sm text-gray-600 mt-1">
                                {param.description}
                              </p>
                            )}
                            {param?.default !== undefined && (
                              <p className="text-xs text-gray-500 mt-1">
                                Default:{" "}
                                <code className="bg-gray-100 px-1 rounded">
                                  {JSON.stringify(param.default)}
                                </code>
                              </p>
                            )}
                            {param?.enum && param.enum.length > 0 && (
                              <p className="text-xs text-gray-500 mt-1">
                                Values:{" "}
                                {param.enum
                                  .map((v: string) => `"${v}"`)
                                  .join(", ")}
                              </p>
                            )}
                          </div>
                        </div>
                      </div>
                    ),
                  )}
                </div>
              </div>
            )}

            {/* Execution Defaults */}
            <ActionDefaultsDisplay action={actionDetails} />
          </div>

          {/* Recent Executions */}
          <div className="bg-white shadow rounded-lg p-6">
            <div className="flex justify-between items-center mb-4">
              <h2 className="text-xl font-semibold">
                Recent Executions ({executions.length})
              </h2>
              <Link
                to={`/executions?action_ref=${action.data?.ref}`}
                className="text-sm text-blue-600 hover:text-blue-800"
              >
                View All →
              </Link>
            </div>
            {executions.length === 0 ? (
              <p className="text-gray-500 text-center py-8">
                No executions yet
              </p>
            ) : (
              <div className="space-y-2">
                {executions.map((execution: ExecutionSummary) => (
                  <Link
                    key={execution.id}
                    to={`/executions/${execution.id}`}
                    className="block p-3 border border-gray-200 rounded hover:bg-gray-50 transition-colors"
                  >
                    <div className="flex justify-between items-center">
                      <div className="flex items-center gap-3">
                        <span className="text-sm font-mono text-gray-600">
                          #{execution.id}
                        </span>
                        <span
                          className={`px-2 py-1 text-xs rounded ${
                            execution.status === "completed"
                              ? "bg-green-100 text-green-800"
                              : execution.status === "failed"
                                ? "bg-red-100 text-red-800"
                                : execution.status === "running"
                                  ? "bg-blue-100 text-blue-800"
                                  : "bg-gray-100 text-gray-800"
                          }`}
                        >
                          {execution.status}
                        </span>
                      </div>
                      <span className="text-xs text-gray-500">
                        {new Date(execution.created).toLocaleString()}
                      </span>
                    </div>
                  </Link>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Sidebar */}
        <div className="space-y-6">
          {/* Quick Stats */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Statistics</h2>
            <div className="space-y-3">
              <div className="flex justify-between items-center">
                <span className="text-sm text-gray-600">Total Executions</span>
                <span className="text-lg font-semibold">
                  {executionsData?.pagination?.total_items || 0}
                </span>
              </div>
              <div className="flex justify-between items-center">
                <span className="text-sm text-gray-600">Recent</span>
                <span className="text-lg font-semibold">
                  {executions.length}
                </span>
              </div>
            </div>
          </div>

          {/* Quick Actions */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Quick Actions</h2>
            <div className="space-y-2">
              <Link
                to={`/packs/${action.data?.pack_ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Pack
              </Link>
              <Link
                to={`/rules?action=${action.data?.ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Rules
              </Link>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function PermissionSetRefChips({ refs }: { refs: string[] }) {
  if (refs.length === 0) {
    return (
      <p className="text-sm text-gray-500">
        No permission set refs configured.
      </p>
    );
  }

  return (
    <div className="flex flex-wrap gap-2">
      {refs.map((ref) => (
        ref === STANDARD_EXECUTION_ACCESS_REF ? (
          <span
            key={ref}
            className="font-mono text-xs px-2 py-1 rounded bg-green-50 text-green-700"
            title="Standard action/pack-scoped keys and artifacts access"
          >
            {ref}
          </span>
        ) : (
          <Link
            key={ref}
            to={`/access-control/permission-sets/${ref}`}
            className="font-mono text-xs px-2 py-1 rounded bg-blue-50 text-blue-700 hover:bg-blue-100"
            title={`View permission set ${ref}`}
          >
            {ref}
          </Link>
        )
      ))}
    </div>
  );
}

function ActionDefaultsDisplay({ action }: { action: ActionResponse }) {
  const currentRefs = action.default_execution_permission_set_refs ?? [];
  const referenceVisibility =
    (action.reference_visibility as ActionReferenceVisibility | undefined) ??
    ActionReferenceVisibility.PUBLIC;
  const allowedPackRefs = action.reference_allowed_pack_refs ?? [];
  const selector = action.worker_selector ?? {};
  const tolerations = (action.worker_tolerations ?? []) as WorkerToleration[];
  const affinity = (action.worker_affinity ?? {}) as WorkerAffinity;
  const selectorEntries = Object.entries(selector);
  const hasAffinity =
    (affinity.required?.length ?? 0) > 0 ||
    (affinity.preferred?.length ?? 0) > 0 ||
    (affinity.anti_affinity?.length ?? 0) > 0;
  const hasRetention =
    Boolean(action.log_retention_policy && action.log_retention_limit) ||
    Boolean(action.artifact_retention_policy && action.artifact_retention_limit);
  return (
    <div className="mt-6 border-t border-gray-200 pt-6">
      <h3 className="text-sm font-medium text-gray-900 mb-4">
        Execution Defaults
      </h3>

      <div className="space-y-4">
        <div>
          <dt className="text-sm font-medium text-gray-500 mb-1">
            Reference Visibility
          </dt>
          <dd>
            <span className="text-xs px-2 py-1 rounded bg-slate-100 text-slate-700 capitalize">
              {referenceVisibility}
            </span>
            {referenceVisibility === "restricted" && allowedPackRefs.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1">
                {allowedPackRefs.map((packRef) => (
                  <span
                    key={packRef}
                    className="font-mono text-xs px-2 py-1 rounded bg-amber-50 text-amber-700"
                  >
                    {packRef}
                  </span>
                ))}
              </div>
            )}
          </dd>
        </div>

        {/* Accesses MCP */}
        {action.accesses_mcp && (
          <div>
            <dt className="text-sm font-medium text-gray-500 mb-1">MCP Access</dt>
            <dd>
              <span className="text-xs px-2 py-1 rounded bg-purple-50 text-purple-700">
                Accesses MCP
              </span>
            </dd>
          </div>
        )}

        {/* Token Access */}
        {currentRefs.length > 0 && (
          <div>
            <dt className="text-sm font-medium text-gray-500 mb-1">
              Default Token Access
            </dt>
            <dd>
              <PermissionSetRefChips refs={currentRefs} />
            </dd>
          </div>
        )}

        {/* Retention */}
        {hasRetention && (
          <div>
            <dt className="text-sm font-medium text-gray-500 mb-1">
              Retention Defaults
            </dt>
            <dd className="flex flex-wrap gap-2">
              {action.log_retention_policy && action.log_retention_limit && (
                <span className="text-xs px-2 py-1 rounded bg-slate-50 text-slate-700">
                  Logs:{" "}
                  {formatRetention(
                    action.log_retention_policy as RetentionPolicy,
                    action.log_retention_limit,
                  )}
                </span>
              )}
              {action.artifact_retention_policy &&
                action.artifact_retention_limit && (
                  <span className="text-xs px-2 py-1 rounded bg-teal-50 text-teal-700">
                    Non-log artifacts:{" "}
                    {formatRetention(
                      action.artifact_retention_policy as RetentionPolicy,
                      action.artifact_retention_limit,
                    )}
                  </span>
                )}
            </dd>
          </div>
        )}

        {/* Worker Selector */}
        {selectorEntries.length > 0 && (
          <div>
            <dt className="text-sm font-medium text-gray-500 mb-1">
              Worker Selector
            </dt>
            <dd className="flex flex-wrap gap-2">
              {selectorEntries.map(([key, value]) => (
                <span
                  key={key}
                  className="font-mono text-xs px-2 py-1 rounded bg-indigo-50 text-indigo-700"
                >
                  {key}={String(value)}
                </span>
              ))}
            </dd>
          </div>
        )}

        {/* Worker Tolerations */}
        {tolerations.length > 0 && (
          <div>
            <dt className="text-sm font-medium text-gray-500 mb-1">
              Worker Tolerations
            </dt>
            <dd className="space-y-1">
              {tolerations.map((t, i) => (
                <div
                  key={i}
                  className="inline-block font-mono text-xs px-2 py-1 rounded bg-amber-50 text-amber-700 mr-2"
                >
                  {t.key}
                  {t.operator === "exists" ? " exists" : `=${t.value ?? "*"}`}
                  {t.effect ? ` (${t.effect})` : ""}
                </div>
              ))}
            </dd>
          </div>
        )}

        {/* Worker Affinity */}
        {hasAffinity && (
          <div>
            <dt className="text-sm font-medium text-gray-500 mb-1">
              Worker Affinity
            </dt>
            <dd>
              <div className="space-y-2">
                {(affinity.required?.length ?? 0) > 0 && (
                  <div>
                    <span className="text-xs font-medium text-gray-500 mr-2">Required:</span>
                    {affinity.required!.map((term, i) => (
                      <span key={i} className="inline-block mr-2">
                        {Object.entries(term.match_labels ?? {}).map(([k, v]) => (
                          <span
                            key={k}
                            className="font-mono text-xs px-2 py-1 rounded bg-green-50 text-green-700 mr-1"
                          >
                            {k}={v}
                          </span>
                        ))}
                      </span>
                    ))}
                  </div>
                )}
                {(affinity.preferred?.length ?? 0) > 0 && (
                  <div>
                    <span className="text-xs font-medium text-gray-500 mr-2">Preferred:</span>
                    {affinity.preferred!.map((pt, i) => (
                      <span key={i} className="inline-block mr-2">
                        <span className="text-xs text-gray-400 mr-1">
                          (w:{pt.weight ?? 1})
                        </span>
                        {Object.entries(pt.preference?.match_labels ?? {}).map(([k, v]) => (
                          <span
                            key={k}
                            className="font-mono text-xs px-2 py-1 rounded bg-blue-50 text-blue-700 mr-1"
                          >
                            {k}={v}
                          </span>
                        ))}
                      </span>
                    ))}
                  </div>
                )}
                {(affinity.anti_affinity?.length ?? 0) > 0 && (
                  <div>
                    <span className="text-xs font-medium text-gray-500 mr-2">Anti-Affinity:</span>
                    {affinity.anti_affinity!.map((term, i) => (
                      <span key={i} className="inline-block mr-2">
                        {Object.entries(term.match_labels ?? {}).map(([k, v]) => (
                          <span
                            key={k}
                            className="font-mono text-xs px-2 py-1 rounded bg-red-50 text-red-700 mr-1"
                          >
                            {k}={v}
                          </span>
                        ))}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            </dd>
          </div>
        )}
      </div>
    </div>
  );
}

function ConfigureActionModal({
  action,
  onClose,
}: {
  action: ActionResponse;
  onClose: () => void;
}) {
  const updateAction = useUpdateAction();
  const { data: permissionSets, isLoading: permissionSetsLoading } =
    usePermissionSets();

  const [label, setLabel] = useState(action.label);
  const [description, setDescription] = useState(action.description ?? "");
  const [entrypoint, setEntrypoint] = useState(action.entrypoint);
  const [accessesMcp, setAccessesMcp] = useState(action.accesses_mcp ?? false);
  const [referenceVisibility, setReferenceVisibility] =
    useState<ActionReferenceVisibility>(
      (action.reference_visibility as ActionReferenceVisibility | undefined) ??
        ActionReferenceVisibility.PUBLIC,
    );
  const [allowedPackRefsText, setAllowedPackRefsText] = useState(
    (action.reference_allowed_pack_refs ?? []).join("\n"),
  );
  const [selectedPermRefs, setSelectedPermRefs] = useState<string[]>([
    ...(action.default_execution_permission_set_refs ?? []),
  ]);
  const [selector, setSelector] = useState<Record<string, string>>(
    (action.worker_selector as Record<string, string>) ?? {},
  );
  const [tolerations, setTolerations] = useState<WorkerToleration[]>(
    (action.worker_tolerations as WorkerToleration[]) ?? [],
  );
  const [affinity, setAffinity] = useState<WorkerAffinity>(
    (action.worker_affinity as WorkerAffinity) ?? {},
  );
  const [logRetention, setLogRetention] = useState<{
    policy: RetentionPolicy | null;
    limit: number | null;
  }>({
    policy: (action.log_retention_policy as RetentionPolicy | undefined) ?? null,
    limit: action.log_retention_limit ?? null,
  });
  const [artifactRetention, setArtifactRetention] = useState<{
    policy: RetentionPolicy | null;
    limit: number | null;
  }>({
    policy:
      (action.artifact_retention_policy as RetentionPolicy | undefined) ?? null,
    limit: action.artifact_retention_limit ?? null,
  });
  const [timeoutSeconds, setTimeoutSeconds] = useState<number | null>(
    action.timeout_seconds ?? null,
  );
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [successMessage, setSuccessMessage] = useState<string | null>(null);

  const allPermissionSetRefs = useMemo(
    () => (permissionSets ?? []).map((set: PermissionSetSummary) => set.ref),
    [permissionSets],
  );

  const selectablePermOptions = useMemo(() => {
    const refSet = new Set([
      STANDARD_EXECUTION_ACCESS_REF,
      ...allPermissionSetRefs,
      ...selectedPermRefs,
    ]);
    return Array.from(refSet)
      .sort((a, b) => a.localeCompare(b))
      .map((ref) => ({
        value: ref,
        label:
          ref === STANDARD_EXECUTION_ACCESS_REF
            ? "standard (action/pack-scoped keys and artifacts)"
            : ref,
      }));
  }, [allPermissionSetRefs, selectedPermRefs]);

  const save = async () => {
    setErrorMessage(null);
    setSuccessMessage(null);

    const hasSelector = Object.keys(selector).length > 0;
    const hasTolerations = tolerations.length > 0;
    const hasAffinity =
      (affinity.required?.length ?? 0) > 0 ||
      (affinity.preferred?.length ?? 0) > 0 ||
      (affinity.anti_affinity?.length ?? 0) > 0;

    try {
      const referenceAllowedPackRefs =
        referenceVisibility === "restricted"
          ? allowedPackRefsText
              .split(/[\n,]/)
              .map((ref) => ref.trim())
              .filter(Boolean)
          : [];

      await updateAction.mutateAsync({
        ref: action.ref,
        data: {
          label,
          description: description || null,
          entrypoint,
          accesses_mcp: accessesMcp,
          reference_visibility: referenceVisibility,
          reference_allowed_pack_refs: referenceAllowedPackRefs,
          default_execution_permission_set_refs: selectedPermRefs,
          worker_selector: hasSelector ? selector : null,
          worker_tolerations: hasTolerations ? tolerations : null,
          worker_affinity: hasAffinity ? affinity : null,
          log_retention_policy: logRetention.policy
            ? {
                op: LogRetentionPolicyPatch.op.SET,
                value: logRetention.policy as RetentionPolicyType,
              }
            : ({
                op: "clear",
              } as unknown as UpdateActionRequest["log_retention_policy"]),
          log_retention_limit: logRetention.limit
            ? { op: LogRetentionLimitPatch.op.SET, value: logRetention.limit }
            : ({
                op: "clear",
              } as unknown as UpdateActionRequest["log_retention_limit"]),
          artifact_retention_policy: artifactRetention.policy
            ? {
                op: LogRetentionPolicyPatch.op.SET,
                value: artifactRetention.policy as RetentionPolicyType,
              }
            : ({
                op: "clear",
              } as unknown as UpdateActionRequest["artifact_retention_policy"]),
          artifact_retention_limit: artifactRetention.limit
            ? {
                op: LogRetentionLimitPatch.op.SET,
                value: artifactRetention.limit,
              }
            : ({
                op: "clear",
              } as unknown as UpdateActionRequest["artifact_retention_limit"]),
          timeout_seconds:
            timeoutSeconds && timeoutSeconds > 0
              ? { op: TimeoutSecondsPatch.op.SET, value: timeoutSeconds }
              : ({
                  op: "clear",
                } as unknown as UpdateActionRequest["timeout_seconds"]),
          // Preserve fields we don't edit in this modal
          runtime: action.runtime ?? null,
          required_worker_runtimes: action.required_worker_runtimes ?? {},
          param_schema: action.param_schema ?? null,
          out_schema: action.out_schema ?? null,
        },
      });
      setSuccessMessage("Action defaults saved.");
    } catch (err) {
      setErrorMessage(
        err instanceof Error ? err.message : "Failed to save action defaults",
      );
    }
  };

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
      <div className="bg-white rounded-lg shadow-xl w-full max-w-2xl max-h-[90vh] overflow-y-auto">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b sticky top-0 bg-white z-10">
          <h2 className="text-xl font-bold">Configure Action Defaults</h2>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <div className="px-6 py-4 space-y-5">
          {/* Label */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Label
            </label>
            <input
              type="text"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:ring-blue-500 focus:border-blue-500"
            />
          </div>

          {/* Description */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Description
            </label>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={3}
              className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:ring-blue-500 focus:border-blue-500"
            />
          </div>

          {/* Entrypoint */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Entry Point
            </label>
            <input
              type="text"
              value={entrypoint}
              onChange={(e) => setEntrypoint(e.target.value)}
              className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm font-mono focus:ring-blue-500 focus:border-blue-500"
            />
          </div>

          {/* Reference Visibility */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Reference Visibility
            </label>
            <p className="text-xs text-gray-500 mb-2">
              Controls which packs may reference this action from rules,
              workflows, and work queues.
            </p>
            <select
              value={referenceVisibility}
              onChange={(e) =>
                setReferenceVisibility(
                  e.target.value as ActionReferenceVisibility,
                )
              }
              className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:ring-blue-500 focus:border-blue-500"
            >
              <option value={ActionReferenceVisibility.PUBLIC}>
                Public - any pack may reference
              </option>
              <option value={ActionReferenceVisibility.PRIVATE}>
                Private - only this pack may reference
              </option>
              <option value={ActionReferenceVisibility.RESTRICTED}>
                Restricted - only allow-listed packs may reference
              </option>
            </select>
            {referenceVisibility === "restricted" && (
              <div className="mt-3">
                <label className="block text-xs font-medium text-gray-600 mb-1">
                  Allowed Pack Refs
                </label>
                <textarea
                  value={allowedPackRefsText}
                  onChange={(e) => setAllowedPackRefsText(e.target.value)}
                  rows={3}
                  placeholder={"core\noperations"}
                  className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm font-mono focus:ring-blue-500 focus:border-blue-500"
                />
                <p className="text-xs text-gray-500 mt-1">
                  Enter one pack ref per line, or comma-separated.
                </p>
              </div>
            )}
          </div>

          {/* Accesses MCP */}
          <div className="flex items-center gap-3">
            <input
              type="checkbox"
              id="accesses-mcp"
              checked={accessesMcp}
              onChange={(e) => setAccessesMcp(e.target.checked)}
              className="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
            />
            <label
              htmlFor="accesses-mcp"
              className="text-sm font-medium text-gray-700"
            >
              Accesses MCP
            </label>
            <span className="text-xs text-gray-500">
              Hint that this action may invoke the Attune MCP server
            </span>
          </div>

          {/* Default Execution Token Access */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Default Execution Token Access
            </label>
            <p className="text-xs text-gray-500 mb-2">
              Permission set refs applied to executions when not explicitly
              overridden. Use{" "}
              <span className="font-mono">standard</span> for
              action/pack-scoped key and artifact access.
            </p>
            {permissionSetsLoading ? (
              <p className="text-xs text-gray-500">
                Loading permission sets...
              </p>
            ) : (
              <MultiSelect
                options={selectablePermOptions}
                value={selectedPermRefs}
                onChange={(refs) =>
                  setSelectedPermRefs(
                    refs.sort((a, b) => a.localeCompare(b)),
                  )
                }
                placeholder="Search and select permission sets..."
              />
            )}
          </div>

          {/* Retention Defaults */}
          <div className="border-t pt-5">
            <h3 className="text-sm font-semibold text-gray-900 mb-4">
              Retention Defaults
            </h3>
            <div className="space-y-3">
              <RetentionPolicyControls
                title="Execution logs"
                description="Default retention for stdout/stderr log artifacts produced by this action."
                policy={logRetention.policy}
                limit={logRetention.limit}
                onChange={setLogRetention}
              />
              <RetentionPolicyControls
                title="Non-log artifacts"
                description="Default retention for artifacts created by executions of this action."
                policy={artifactRetention.policy}
                limit={artifactRetention.limit}
                onChange={setArtifactRetention}
              />
            </div>
          </div>

          {/* Execution Timeout Default */}
          <div className="border-t pt-5">
            <h3 className="text-sm font-semibold text-gray-900 mb-4">
              Execution Timeout Default
            </h3>
            <p className="text-xs text-gray-500 mb-2">
              Default timeout (in seconds) snapshotted onto executions of this
              action. Leave empty to inherit the platform default.
            </p>
            <input
              type="number"
              min={1}
              value={timeoutSeconds ?? ""}
              onChange={(e) => {
                const v = e.target.value;
                setTimeoutSeconds(v === "" ? null : Number(v));
              }}
              placeholder="Platform default"
              className="w-48 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
          </div>

          {/* Worker Placement */}
          <div className="border-t pt-5">
            <h3 className="text-sm font-semibold text-gray-900 mb-4">
              Worker Placement Defaults
            </h3>

            <div className="space-y-5">
              <WorkerSelectorEditor
                value={selector}
                onChange={setSelector}
              />

              <WorkerTolerationsEditor
                value={tolerations}
                onChange={setTolerations}
              />

              <WorkerAffinityEditor
                value={affinity}
                onChange={setAffinity}
              />
            </div>
          </div>

          {/* Feedback */}
          {errorMessage && (
            <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
              {errorMessage}
            </div>
          )}
          {successMessage && (
            <div className="rounded-md border border-green-200 bg-green-50 px-3 py-2 text-sm text-green-700">
              {successMessage}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex justify-end gap-3 px-6 py-4 border-t sticky bottom-0 bg-white">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-2 text-sm text-gray-700 bg-gray-100 rounded hover:bg-gray-200"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={save}
            disabled={updateAction.isPending}
            className="px-4 py-2 text-sm text-white bg-blue-600 rounded hover:bg-blue-700 disabled:opacity-50"
          >
            {updateAction.isPending ? "Saving..." : "Save Defaults"}
          </button>
        </div>
      </div>
    </div>
  );
}
