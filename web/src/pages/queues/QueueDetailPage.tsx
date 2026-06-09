import { useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  AlertTriangle,
  ArrowLeft,
  Eye,
  Pencil,
  Plus,
  Trash2,
} from "lucide-react";
import LiveStreamControl, {
  DEFAULT_LIVE_LIST_MAX_ITEMS,
} from "@/components/common/LiveStreamControl";
import OnOffSwitch from "@/components/common/OnOffSwitch";
import Pagination from "@/components/executions/Pagination";
import QueueConfigSummary from "@/components/queues/QueueConfigSummary";
import QueueItemModal from "@/components/queues/QueueItemModal";
import {
  formatDateTime,
  formatJsonPreview,
  getQueueSourceBadge,
  getStatusBadge,
  isMutablePendingStatus,
} from "@/components/queues/queueUtils";
import {
  WorkQueueItemStatus,
  type WorkQueueResponse,
  type WorkQueueItemResponse,
} from "@/api/queues";
import { useAuth } from "@/contexts/AuthContext";
import {
  useDeleteQueue,
  useDeleteQueueItem,
  useQueue,
  useQueueItems,
  useUpdateQueue,
} from "@/hooks/useQueues";
import { useQueueStream } from "@/hooks/useQueueStream";
import { useAction } from "@/hooks/useActions";
import { hasPermission } from "@/lib/permissions";

const STATUS_FILTERS: Array<{
  value: string;
  label: string;
  statuses?: WorkQueueItemStatus[];
}> = [
  { value: "all", label: "All items" },
  {
    value: "pending",
    label: "Pending only",
    statuses: [WorkQueueItemStatus.QUEUED, WorkQueueItemStatus.RETRY],
  },
  {
    value: WorkQueueItemStatus.QUEUED,
    label: "Queued",
    statuses: [WorkQueueItemStatus.QUEUED],
  },
  {
    value: WorkQueueItemStatus.RETRY,
    label: "Retry",
    statuses: [WorkQueueItemStatus.RETRY],
  },
  {
    value: WorkQueueItemStatus.LEASED,
    label: "Leased",
    statuses: [WorkQueueItemStatus.LEASED],
  },
  {
    value: WorkQueueItemStatus.COMPLETED,
    label: "Completed",
    statuses: [WorkQueueItemStatus.COMPLETED],
  },
  {
    value: WorkQueueItemStatus.FAILED,
    label: "Failed",
    statuses: [WorkQueueItemStatus.FAILED],
  },
  {
    value: WorkQueueItemStatus.SKIPPED,
    label: "Skipped",
    statuses: [WorkQueueItemStatus.SKIPPED],
  },
  {
    value: WorkQueueItemStatus.CANCELLED,
    label: "Cancelled",
    statuses: [WorkQueueItemStatus.CANCELLED],
  },
];

function getErrorMessage(error: unknown, fallback: string): string {
  const maybeAxios = error as { response?: { data?: { message?: string } } };
  return (
    maybeAxios.response?.data?.message ||
    (error instanceof Error ? error.message : fallback)
  );
}

interface QueueFlagToggleProps {
  label: string;
  description: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => Promise<void>;
}

function QueueFlagToggle({
  label,
  description,
  checked,
  disabled = false,
  onChange,
}: QueueFlagToggleProps) {
  return (
    <label className="flex items-center justify-between gap-4 rounded-lg border border-gray-200 bg-white px-4 py-3 shadow-sm">
      <div>
        <div className="text-sm font-medium text-gray-900">{label}</div>
        <div className="mt-1 text-xs text-gray-500">{description}</div>
      </div>
      <OnOffSwitch
        checked={checked}
        disabled={disabled}
        ariaLabel={label}
        onChange={(nextChecked) => {
          void onChange(nextChecked);
        }}
      />
    </label>
  );
}

async function updateOperationalFlag(
  updateQueue: ReturnType<typeof useUpdateQueue>,
  queue: WorkQueueResponse,
  patch: { enabled?: boolean; accepting_new_items?: boolean },
) {
  await updateQueue.mutateAsync({
    ref: queue.ref,
    data: patch,
  });
}

export function QueueDetailPage() {
  const { ref } = useParams<{ ref: string }>();
  const queueRef = ref ?? "";
  const navigate = useNavigate();
  const { user } = useAuth();

  const [page, setPage] = useState(1);
  const [itemKeyFilter, setItemKeyFilter] = useState("");
  const [enqueueSourceFilter, setEnqueueSourceFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState("pending");
  const [showCreateItemModal, setShowCreateItemModal] = useState(false);
  const [editingItem, setEditingItem] = useState<WorkQueueItemResponse | null>(
    null,
  );
  const [actionError, setActionError] = useState<string | null>(null);
  const [showRawConfigJson, setShowRawConfigJson] = useState(false);
  const [livePaused, setLivePaused] = useState(false);
  const pageSize = 20;

  const { data, isLoading, error } = useQueue(queueRef);
  const updateQueue = useUpdateQueue();
  const { isConnected: isQueueStreamConnected } = useQueueStream({
    queueRef,
    paused: livePaused,
  });
  const queue = data?.data;
  const { data: actionData } = useAction(queue?.dispatch_action_ref || "");
  const statuses = useMemo(
    () =>
      STATUS_FILTERS.find((filter) => filter.value === statusFilter)?.statuses,
    [statusFilter],
  );

  const {
    data: itemsData,
    isLoading: isItemsLoading,
    error: itemsError,
    isFetching: isItemsFetching,
  } = useQueueItems(queueRef, {
    page,
    pageSize,
    itemKey: itemKeyFilter.trim() || undefined,
    enqueueSource: enqueueSourceFilter.trim() || undefined,
    statuses,
  });

  const deleteQueue = useDeleteQueue();
  const deleteQueueItem = useDeleteQueueItem();

  const items = itemsData?.items ?? [];
  const itemPagination = itemsData?.pagination;
  const itemTotal = itemPagination?.total_items ?? 0;
  const sourceBadge = queue ? getQueueSourceBadge(queue.is_adhoc) : null;
  const canUpdateQueues = hasPermission(user, "queues", "update");
  const canUpdateQueuesResolved = true;
  const queueFlagControlsDisabled =
    !queue ||
    updateQueue.isPending ||
    (canUpdateQueuesResolved && !canUpdateQueues);

  const clearItemFilters = () => {
    setItemKeyFilter("");
    setEnqueueSourceFilter("");
    setStatusFilter("pending");
    setPage(1);
  };

  const handleDeleteQueue = async () => {
    if (!queue) {
      return;
    }
    if (!window.confirm(`Delete queue "${queue.ref}"?`)) {
      return;
    }

    try {
      await deleteQueue.mutateAsync(queue.ref);
      navigate("/queues");
    } catch (deleteError) {
      setActionError(getErrorMessage(deleteError, "Failed to delete queue"));
    }
  };

  const handleDeleteItem = async (item: WorkQueueItemResponse) => {
    if (!window.confirm(`Delete pending queue item #${item.id}?`)) {
      return;
    }

    try {
      await deleteQueueItem.mutateAsync({ ref: queueRef, itemId: item.id });
      setActionError(null);
    } catch (deleteError) {
      setActionError(
        getErrorMessage(deleteError, "Failed to delete queue item"),
      );
    }
  };

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex h-64 items-center justify-center">
          <div className="h-12 w-12 animate-spin rounded-full border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error || !queue) {
    return (
      <div className="mx-auto max-w-5xl p-6">
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-red-700">
          {error instanceof Error ? error.message : "Queue not found"}
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 pb-20">
      <div className="mb-6 flex items-start justify-between gap-4">
        <div>
          <Link
            to="/queues"
            className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900"
          >
            <ArrowLeft className="mr-1 h-4 w-4" />
            Back to Queues
          </Link>
          <div className="mt-4 flex flex-wrap items-center gap-3">
            <h1 className="text-3xl font-bold text-gray-900">{queue.label}</h1>
            <LiveStreamControl
              paused={livePaused}
              onTogglePaused={() => setLivePaused((paused) => !paused)}
              connected={isQueueStreamConnected}
              maxItems={DEFAULT_LIVE_LIST_MAX_ITEMS}
              itemLabel="queue updates"
              showRetentionHint={false}
            />
            {sourceBadge && (
              <span
                className={`inline-flex rounded-full px-2.5 py-1 text-xs font-semibold ${sourceBadge.classes}`}
              >
                {sourceBadge.label}
              </span>
            )}
            <span className="inline-flex rounded-full bg-gray-100 px-2.5 py-1 text-xs font-semibold capitalize text-gray-700">
              {queue.reference_visibility}
            </span>
          </div>
          <p className="mt-2 font-mono text-sm text-gray-500">{queue.ref}</p>
          {queue.reference_visibility === "restricted" && (
            <p className="mt-2 text-sm text-gray-500">
              Allowed referencing packs:{" "}
              <span className="font-mono">
                {queue.reference_allowed_pack_refs?.length
                  ? queue.reference_allowed_pack_refs.join(", ")
                  : "none"}
              </span>
            </p>
          )}
          <p className="mt-2 max-w-3xl text-gray-600">
            {queue.description || "No description provided."}
          </p>
          <div className="mt-4 grid gap-3 md:grid-cols-2">
            <QueueFlagToggle
              label="Processing enabled"
              description="Allow the executor to lease and process queued items."
              checked={queue.enabled}
              disabled={queueFlagControlsDisabled}
              onChange={async (checked) => {
                try {
                  await updateOperationalFlag(updateQueue, queue, {
                    enabled: checked,
                  });
                  setActionError(null);
                } catch (toggleError) {
                  setActionError(
                    getErrorMessage(toggleError, "Failed to update queue"),
                  );
                }
              }}
            />
            <QueueFlagToggle
              label="Accepting items"
              description="Allow new queue items to be inserted through the API and UI."
              checked={queue.accepting_new_items}
              disabled={queueFlagControlsDisabled}
              onChange={async (checked) => {
                try {
                  await updateOperationalFlag(updateQueue, queue, {
                    accepting_new_items: checked,
                  });
                  setActionError(null);
                } catch (toggleError) {
                  setActionError(
                    getErrorMessage(toggleError, "Failed to update queue"),
                  );
                }
              }}
            />
          </div>
          {canUpdateQueuesResolved && !canUpdateQueues && (
            <p className="mt-3 text-sm text-gray-500">
              Queue status controls require the{" "}
              <span className="font-mono">queues:update</span> permission.
            </p>
          )}
        </div>

        <div className="flex items-center gap-2">
          {queue.is_adhoc && (
            <Link
              to={`/queues/${encodeURIComponent(queue.ref)}/edit`}
              className="inline-flex items-center gap-2 rounded-lg bg-white px-4 py-2 text-gray-700 shadow hover:bg-gray-50"
            >
              <Pencil className="h-4 w-4" />
              Edit Queue
            </Link>
          )}
          <button
            type="button"
            onClick={() => setShowCreateItemModal(true)}
            disabled={!queue.accepting_new_items}
            className="inline-flex items-center gap-2 rounded-lg bg-blue-600 px-4 py-2 text-white hover:bg-blue-700 transition-colors disabled:cursor-not-allowed disabled:bg-gray-300"
          >
            <Plus className="h-4 w-4" />
            Add Queue Item
          </button>
          {queue.is_adhoc && (
            <button
              type="button"
              onClick={handleDeleteQueue}
              className="inline-flex items-center gap-2 rounded-lg bg-red-50 px-4 py-2 text-red-700 hover:bg-red-100 transition-colors"
            >
              <Trash2 className="h-4 w-4" />
              Delete Queue
            </button>
          )}
        </div>
      </div>

      {!queue.is_adhoc && sourceBadge && (
        <div className="mb-6 rounded-lg border border-purple-200 bg-purple-50 p-4 text-sm text-purple-900">
          {sourceBadge.description}
        </div>
      )}

      {!queue.accepting_new_items && (
        <div className="mb-6 rounded-lg border border-amber-200 bg-amber-50 p-4 text-sm text-amber-900">
          New queue items are currently blocked for this queue.
        </div>
      )}

      {actionError && (
        <div className="mb-6 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-red-700">
          {actionError}
        </div>
      )}

      <div className="mb-6 rounded-lg bg-white p-5 shadow">
        <div className="mb-4 flex items-center justify-between gap-4">
          <div>
            <h2 className="text-lg font-semibold text-gray-900">Queue items</h2>
            <p className="mt-1 text-sm text-gray-500">
              Pending items can be edited or deleted while they remain queued or
              retrying.
            </p>
          </div>
          <div className="text-sm text-gray-600">
            {itemTotal > 0
              ? `${itemTotal} total item${itemTotal === 1 ? "" : "s"}`
              : "No items yet"}
            {isItemsFetching && !isItemsLoading ? " • refreshing…" : ""}
          </div>
        </div>

        <div className="mb-4 grid gap-4 md:grid-cols-3">
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Item key search
            </label>
            <input
              value={itemKeyFilter}
              onChange={(e) => {
                setItemKeyFilter(e.target.value);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="order-123"
            />
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Enqueue source
            </label>
            <input
              value={enqueueSourceFilter}
              onChange={(e) => {
                setEnqueueSourceFilter(e.target.value);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="api"
            />
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Status filter
            </label>
            <select
              value={statusFilter}
              onChange={(e) => {
                setStatusFilter(e.target.value);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              {STATUS_FILTERS.map((filter) => (
                <option key={filter.value} value={filter.value}>
                  {filter.label}
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="mb-4 flex justify-end">
          <button
            type="button"
            onClick={clearItemFilters}
            className="text-sm text-gray-600 hover:text-gray-900"
          >
            Reset item filters
          </button>
        </div>

        {isItemsLoading ? (
          <div className="py-12 text-center">
            <div className="inline-block h-8 w-8 animate-spin rounded-full border-b-2 border-blue-600" />
            <p className="mt-4 text-gray-600">Loading queue items...</p>
          </div>
        ) : itemsError ? (
          <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-red-700">
            {itemsError instanceof Error
              ? itemsError.message
              : "Failed to load queue items"}
          </div>
        ) : items.length === 0 ? (
          <div className="rounded-lg border border-dashed border-gray-300 px-6 py-12 text-center">
            <Eye className="mx-auto h-10 w-10 text-gray-400" />
            <p className="mt-4 text-gray-600">
              No queue items match the current filters.
            </p>
          </div>
        ) : (
          <>
            <div className="overflow-x-auto">
              <table className="min-w-full divide-y divide-gray-200">
                <thead className="bg-gray-50">
                  <tr>
                    <th className="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      ID / key
                    </th>
                    <th className="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Status
                    </th>
                    <th className="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Priority
                    </th>
                    <th className="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Payload
                    </th>
                    <th className="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Metadata
                    </th>
                    <th className="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Requested / updated
                    </th>
                    <th className="px-4 py-3 text-right text-xs font-medium uppercase tracking-wider text-gray-500">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-200 bg-white">
                  {items.map((item) => {
                    const statusBadge = getStatusBadge(item.status);
                    const isMutable = isMutablePendingStatus(item.status);
                    return (
                      <tr key={item.id} className="hover:bg-gray-50">
                        <td className="px-4 py-4 align-top">
                          <div className="text-sm font-semibold text-gray-900">
                            #{item.id}
                          </div>
                          <div className="mt-1 font-mono text-xs text-gray-500">
                            {item.item_key || "—"}
                          </div>
                          <div className="mt-1 text-xs text-gray-500">
                            Source: {item.enqueue_source}
                          </div>
                        </td>
                        <td className="px-4 py-4 align-top whitespace-nowrap">
                          <span
                            className={`inline-flex rounded-full px-2 py-1 text-xs font-semibold ${statusBadge.classes}`}
                          >
                            {statusBadge.label}
                          </span>
                          {!isMutable && (
                            <div className="mt-2 inline-flex items-center gap-1 text-xs text-gray-500">
                              <AlertTriangle className="h-3.5 w-3.5" />
                              Read-only now
                            </div>
                          )}
                        </td>
                        <td className="px-4 py-4 align-top text-sm text-gray-700">
                          <div>{item.priority}</div>
                          <div className="mt-1 text-xs text-gray-500">
                            Attempts: {item.attempt_count}
                          </div>
                        </td>
                        <td className="px-4 py-4 align-top">
                          <pre className="max-w-xs whitespace-pre-wrap break-words text-xs text-gray-700">
                            {formatJsonPreview(item.payload)}
                          </pre>
                        </td>
                        <td className="px-4 py-4 align-top">
                          <pre className="max-w-xs whitespace-pre-wrap break-words text-xs text-gray-700">
                            {formatJsonPreview(item.metadata)}
                          </pre>
                        </td>
                        <td className="px-4 py-4 align-top text-xs text-gray-600">
                          <div>Created: {formatDateTime(item.created)}</div>
                          <div className="mt-1">
                            Updated: {formatDateTime(item.updated)}
                          </div>
                          {item.lease_expires_at && (
                            <div className="mt-1">
                              Lease expires:{" "}
                              {formatDateTime(item.lease_expires_at)}
                            </div>
                          )}
                        </td>
                        <td className="px-4 py-4 align-top text-right">
                          <div className="flex items-center justify-end gap-2">
                            {isMutable ? (
                              <>
                                <button
                                  type="button"
                                  onClick={() => setEditingItem(item)}
                                  className="text-gray-500 hover:text-blue-600"
                                  title="Edit pending item"
                                >
                                  <Pencil className="h-4 w-4" />
                                </button>
                                <button
                                  type="button"
                                  onClick={() => handleDeleteItem(item)}
                                  className="text-gray-500 hover:text-red-600"
                                  title="Delete pending item"
                                >
                                  <Trash2 className="h-4 w-4" />
                                </button>
                              </>
                            ) : (
                              <span className="text-xs text-gray-400">
                                Immutable
                              </span>
                            )}
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </>
        )}
      </div>

      <div className="rounded-lg bg-white p-5 shadow">
        <div className="mb-4 flex items-start justify-between gap-4">
          <div>
            <h2 className="text-lg font-semibold text-gray-900">
              Queue config
            </h2>
            <p className="mt-1 text-sm text-gray-500">
              {showRawConfigJson
                ? "Inspect the persisted queue item schema, action params, and queue config JSON."
                : "Dispatch behaviour, action parameter mappings, and tunables for this queue."}
            </p>
          </div>
          <button
            type="button"
            onClick={() => setShowRawConfigJson((current) => !current)}
            className="inline-flex shrink-0 items-center gap-2 rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50"
          >
            {showRawConfigJson ? "Show structured view" : "Show raw JSON"}
          </button>
        </div>
        <QueueConfigSummary
          queue={queue}
          dispatchActionParamSchema={actionData?.data?.param_schema}
          showRawJson={showRawConfigJson}
        />
      </div>

      <Pagination
        page={page}
        setPage={setPage}
        pageSize={pageSize}
        itemCount={items.length}
        total={itemTotal}
        hasPrevious={itemPagination?.has_previous}
        hasNext={itemPagination?.has_next}
        itemLabel="queue items"
      />

      {showCreateItemModal && (
        <QueueItemModal
          queueRef={queue.ref}
          itemSchema={queue.item_schema}
          defaultPriority={queue.default_priority}
          onClose={() => setShowCreateItemModal(false)}
        />
      )}
      {editingItem && (
        <QueueItemModal
          queueRef={queue.ref}
          itemSchema={queue.item_schema}
          item={editingItem}
          onClose={() => setEditingItem(null)}
        />
      )}
    </div>
  );
}

export default QueueDetailPage;
