import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Plus, Search, Workflow } from "lucide-react";
import OnOffSwitch from "@/components/common/OnOffSwitch";
import Pagination from "@/components/executions/Pagination";
import QueueInspectionPreview from "@/components/queues/QueueInspectionPreview";
import QueueUpNextList from "@/components/queues/QueueUpNextList";
import type { WorkQueueSummary } from "@/api/queues";
import { useAuth } from "@/contexts/AuthContext";
import { useActions } from "@/hooks/useActions";
import { useQueueStream } from "@/hooks/useQueueStream";
import { useQueue, useQueues, useUpdateQueue } from "@/hooks/useQueues";
import { hasPermission } from "@/lib/permissions";
import PackIcon from "@/components/common/PackIcon";
import { packRefFromComponentRef } from "@/utils/packIcons";

function getMutationErrorMessage(error: unknown): string {
  const maybeApiError = error as { body?: { message?: string } };
  const maybeAxios = error as { response?: { data?: { message?: string } } };
  return (
    maybeApiError.body?.message ||
    maybeAxios.response?.data?.message ||
    (error instanceof Error ? error.message : "Failed to update queue")
  );
}

interface QueueFlagToggleProps {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => Promise<void>;
}

function QueueFlagToggle({
  label,
  checked,
  disabled = false,
  onChange,
}: QueueFlagToggleProps) {
  return (
    <div className="flex items-center gap-2 text-xs text-gray-700">
      <OnOffSwitch
        checked={checked}
        disabled={disabled}
        ariaLabel={label}
        stopPropagation
        onChange={(nextChecked) => {
          void onChange(nextChecked);
        }}
      />
      <span>{label}</span>
    </div>
  );
}

export default function QueuesPage() {
  const [page, setPage] = useState(1);
  const [search, setSearch] = useState("");
  const [enabledFilter, setEnabledFilter] = useState<
    "all" | "enabled" | "disabled"
  >("all");
  const [managementFilter, setManagementFilter] = useState<
    "all" | "api" | "pack"
  >("all");
  const [preferredQueueRef, setPreferredQueueRef] = useState("");
  const [statusError, setStatusError] = useState<string | null>(null);
  const pageSize = 20;
  const { user } = useAuth();

  const queryParams = useMemo(
    () => ({
      page,
      pageSize,
      search: search.trim() || undefined,
      enabled:
        enabledFilter === "all" ? undefined : enabledFilter === "enabled",
      isAdhoc:
        managementFilter === "all" ? undefined : managementFilter === "api",
    }),
    [enabledFilter, managementFilter, page, search],
  );

  const { data, isLoading, error, isFetching } = useQueues(queryParams);
  const { data: actionsData } = useActions({ pageSize: 1000 });
  const updateQueue = useUpdateQueue();
  useQueueStream();

  const queues = data?.items ?? [];
  const actionDescriptionsByRef = useMemo(
    () =>
      new Map(
        (actionsData?.items ?? []).map((action) => [
          action.ref,
          action.description,
        ]),
      ),
    [actionsData?.items],
  );
  const selectedQueueRef = queues.some(
    (queue) => queue.ref === preferredQueueRef,
  )
    ? preferredQueueRef
    : (queues[0]?.ref ?? "");
  const {
    data: selectedQueueData,
    isLoading: isSelectedQueueLoading,
    error: selectedQueueError,
  } = useQueue(selectedQueueRef);
  const pagination = data?.pagination;
  const total = pagination?.total_items ?? 0;
  const hasActiveFilters =
    search.trim().length > 0 ||
    enabledFilter !== "all" ||
    managementFilter !== "all";

  const selectedQueue = selectedQueueData?.data;
  const canUpdateQueues = hasPermission(user, "queues", "update");
  const canUpdateQueuesResolved = true;

  const clearFilters = () => {
    setSearch("");
    setEnabledFilter("all");
    setManagementFilter("all");
    setPage(1);
  };

  const updateOperationalFlag = async (
    queue: WorkQueueSummary,
    patch: { enabled?: boolean; accepting_new_items?: boolean },
  ) => {
    setStatusError(null);
    try {
      await updateQueue.mutateAsync({
        ref: queue.ref,
        data: patch,
      });
    } catch (mutationError) {
      setStatusError(getMutationErrorMessage(mutationError));
    }
  };

  return (
    <div className="p-6 pb-28">
      <div className="mb-6 flex items-center justify-between gap-4">
        <div>
          <h1 className="text-3xl font-bold text-gray-900">Work Queues</h1>
          <p className="mt-2 text-gray-600">
            Browse queue definitions, inspect queue state, and manage queue
            processing.
          </p>
        </div>
        <Link
          to="/queues/new"
          className="inline-flex items-center gap-2 rounded-lg bg-blue-600 px-4 py-2 text-white transition-colors hover:bg-blue-700"
        >
          <Plus className="h-4 w-4" />
          Create Queue
        </Link>
      </div>

      <div className="mb-6 rounded-lg bg-white p-4 shadow">
        <div className="grid gap-4 md:grid-cols-3">
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              <span className="inline-flex items-center gap-2">
                <Search className="h-4 w-4" />
                Search queues
              </span>
            </label>
            <input
              value={search}
              onChange={(event) => {
                setSearch(event.target.value);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Search by ref, label, or description"
            />
          </div>

          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Processing state
            </label>
            <select
              value={enabledFilter}
              onChange={(event) => {
                setEnabledFilter(event.target.value as typeof enabledFilter);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="all">All queues</option>
              <option value="enabled">Processing enabled</option>
              <option value="disabled">Processing paused</option>
            </select>
          </div>

          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Management source
            </label>
            <select
              value={managementFilter}
              onChange={(event) => {
                setManagementFilter(
                  event.target.value as typeof managementFilter,
                );
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="all">All queues</option>
              <option value="api">API-managed</option>
              <option value="pack">Pack-managed</option>
            </select>
          </div>
        </div>

        <div className="mt-4 flex items-center justify-between">
          <div className="text-sm text-gray-600">
            {queues.length > 0
              ? `Showing ${queues.length} of ${total} queue${total === 1 ? "" : "s"}`
              : "No queues found"}
            {isFetching && !isLoading ? " • refreshing…" : ""}
          </div>
          {hasActiveFilters && (
            <button
              type="button"
              onClick={clearFilters}
              className="text-sm text-gray-600 hover:text-gray-900"
            >
              Clear filters
            </button>
          )}
        </div>
      </div>

      {statusError && (
        <div className="mb-6 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {statusError}
        </div>
      )}

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1.8fr)_360px]">
        <div>
          <div className="mb-3 text-sm text-gray-600">
            Select a queue row to inspect it. Use the inline toggles to control
            inserts and executor processing.
          </div>
          {canUpdateQueuesResolved && !canUpdateQueues && (
            <div className="mb-3 rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900">
              Queue status controls require the{" "}
              <span className="font-mono">queues:update</span> permission.
            </div>
          )}

          <div className="overflow-hidden rounded-lg bg-white shadow">
            {isLoading ? (
              <div className="p-12 text-center">
                <div className="inline-block h-8 w-8 animate-spin rounded-full border-b-2 border-blue-600" />
                <p className="mt-4 text-gray-600">Loading queues...</p>
              </div>
            ) : error ? (
              <div className="p-12 text-center">
                <p className="text-red-600">Failed to load queues</p>
                <p className="mt-2 text-sm text-gray-600">
                  {error instanceof Error ? error.message : "Unknown error"}
                </p>
              </div>
            ) : queues.length === 0 ? (
              <div className="p-12 text-center">
                <Workflow className="mx-auto h-12 w-12 text-gray-400" />
                <p className="mt-4 text-gray-600">No queues found</p>
                <p className="mt-1 text-sm text-gray-500">
                  {hasActiveFilters
                    ? "Try adjusting your filters."
                    : "Create your first API-managed queue to get started."}
                </p>
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="min-w-full table-fixed divide-y divide-gray-200">
                  <thead className="bg-gray-50">
                    <tr>
                      <th className="w-[40%] px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                        Queue
                      </th>
                      <th className="w-[30%] px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                        Enablement
                      </th>
                      <th className="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                        Dispatch action
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-gray-200 bg-white">
                    {queues.map((queue) => {
                      const isSelected = queue.ref === selectedQueueRef;
                      const isUpdating =
                        updateQueue.isPending &&
                        updateQueue.variables?.ref === queue.ref;
                      const actionDescription = actionDescriptionsByRef.get(
                        queue.dispatch_action_ref,
                      );

                      return (
                        <tr
                          key={queue.id}
                          tabIndex={0}
                          onClick={() => setPreferredQueueRef(queue.ref)}
                          onKeyDown={(event) => {
                            if (event.key === "Enter" || event.key === " ") {
                              event.preventDefault();
                              setPreferredQueueRef(queue.ref);
                            }
                          }}
                          className={`cursor-pointer ${
                            isSelected ? "bg-blue-50" : "hover:bg-gray-50"
                          }`}
                        >
                          <td className="px-4 py-4">
                            <div className="flex min-w-0 items-start gap-2">
                              <PackIcon
                                packRef={queue.pack_ref}
                                size="sm"
                                className="mt-0.5"
                              />
                              <div className="min-w-0">
                                <Link
                                  to={`/queues/${encodeURIComponent(queue.ref)}`}
                                  onClick={() =>
                                    setPreferredQueueRef(queue.ref)
                                  }
                                  className={`block truncate text-sm font-medium hover:underline ${
                                    isSelected
                                      ? "text-blue-700"
                                      : "text-blue-600"
                                  }`}
                                >
                                  {queue.pack_ref
                                    ? `${queue.pack_ref}: ${queue.label}`
                                    : queue.label}
                                </Link>
                                <div className="truncate text-xs font-mono text-gray-500">
                                  {queue.ref}
                                </div>
                                <div className="mt-2 flex flex-wrap items-center gap-2">
                                  <span className="inline-flex rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium capitalize text-gray-700">
                                    {queue.reference_visibility}
                                  </span>
                                  {queue.reference_visibility ===
                                    "restricted" &&
                                  queue.reference_allowed_pack_refs?.length ? (
                                    <span className="text-xs text-gray-500">
                                      Allowed:{" "}
                                      {queue.reference_allowed_pack_refs.join(
                                        ", ",
                                      )}
                                    </span>
                                  ) : null}
                                </div>
                                {queue.description && (
                                  <p className="mt-1 whitespace-normal break-words text-sm text-gray-600">
                                    {queue.description}
                                  </p>
                                )}
                              </div>
                            </div>
                          </td>
                          <td className="px-4 py-4 align-top">
                            <div className="space-y-2">
                              <QueueFlagToggle
                                label="Accept new items"
                                checked={queue.accepting_new_items}
                                disabled={isUpdating || !canUpdateQueues}
                                onChange={async (checked) =>
                                  updateOperationalFlag(queue, {
                                    accepting_new_items: checked,
                                  })
                                }
                              />
                              <QueueFlagToggle
                                label="Executor processing"
                                checked={queue.enabled}
                                disabled={isUpdating || !canUpdateQueues}
                                onChange={async (checked) =>
                                  updateOperationalFlag(queue, {
                                    enabled: checked,
                                  })
                                }
                              />
                            </div>
                            {isUpdating && (
                              <div className="mt-2 text-xs text-gray-500">
                                Saving…
                              </div>
                            )}
                          </td>
                          <td className="px-4 py-4 text-sm font-mono text-gray-700">
                            <div className="flex items-center gap-2">
                              <PackIcon
                                packRef={packRefFromComponentRef(
                                  queue.dispatch_action_ref,
                                )}
                                size="sm"
                              />
                              <div className="truncate">
                                {queue.dispatch_action_ref}
                              </div>
                            </div>
                            {actionDescription && (
                              <div className="mt-1 whitespace-normal break-words text-xs text-gray-500">
                                {actionDescription}
                              </div>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </div>

          <Pagination
            page={page}
            setPage={setPage}
            pageSize={pageSize}
            itemCount={queues.length}
            total={total}
            hasPrevious={pagination?.has_previous}
            hasNext={pagination?.has_next}
            itemLabel="queues"
            floating
          />
        </div>

        <div className="space-y-6 xl:sticky xl:top-6 xl:self-start">
          {!selectedQueueRef ? (
            <div className="rounded-lg border border-dashed border-gray-300 bg-white px-5 py-10 text-center text-sm text-gray-500 shadow">
              Select a queue from the list to inspect it.
            </div>
          ) : isSelectedQueueLoading ? (
            <div className="rounded-lg bg-white py-10 text-center shadow">
              <div className="inline-block h-8 w-8 animate-spin rounded-full border-b-2 border-blue-600" />
              <p className="mt-4 text-sm text-gray-600">
                Loading queue configuration...
              </p>
            </div>
          ) : selectedQueueError || !selectedQueue ? (
            <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700 shadow">
              {selectedQueueError instanceof Error
                ? selectedQueueError.message
                : "Failed to load queue details"}
            </div>
          ) : (
            <QueueInspectionPreview queue={selectedQueue} />
          )}

          {selectedQueue && <QueueUpNextList queueRef={selectedQueue.ref} />}
        </div>
      </div>
    </div>
  );
}
