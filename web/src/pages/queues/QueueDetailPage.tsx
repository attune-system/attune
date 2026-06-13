import { useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  AlertTriangle,
  ArrowLeft,
  Eye,
  ExternalLink,
  Filter,
  Pencil,
  Plus,
  Trash2,
  X,
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
  parseJsonObject,
} from "@/components/queues/queueUtils";
import {
  WorkQueueItemBulkOperation,
  WorkQueueItemStatus,
  type JsonValue,
  type ApplyWorkQueueItemsRequest,
  type ApplyWorkQueueItemsResponse,
  type PreviewWorkQueueItemsResponse,
  type WorkQueueResponse,
  type WorkQueueItemResponse,
} from "@/api/queues";
import { useAuth } from "@/contexts/AuthContext";
import {
  useDeleteQueue,
  useDeleteQueueItem,
  useApplyQueueItemsBySelector,
  usePreviewQueueItemsBySelector,
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
  const maybeAxios = error as { response?: { data?: { error?: string; message?: string } } };
  return (
    maybeAxios.response?.data?.error ||
    maybeAxios.response?.data?.message ||
    (error instanceof Error ? error.message : fallback)
  );
}

type SelectorVariableType = "string" | "number" | "boolean" | "null" | "json";
type SelectorInputMode = "condition" | "raw";

interface SelectorVariableRow {
  id: string;
  name: string;
  type: SelectorVariableType;
  value: string;
}

interface PendingBulkConfirmation {
  request: ApplyWorkQueueItemsRequest;
  matchedCount: number;
}

interface BulkConfirmationCopy {
  title: string;
  actionPhrase: string;
  remainder: string;
  emphasizedRemainder?: string;
  confirmLabel: string;
}

function createSelectorVariableRow(): SelectorVariableRow {
  return {
    id: crypto.randomUUID(),
    name: "",
    type: "string",
    value: "",
  };
}

function getBulkConfirmationCopy(
  operation: WorkQueueItemBulkOperation,
  matchedCount: number,
  priority?: number | null,
): BulkConfirmationCopy {
  const itemLabel = `pending queue item${matchedCount === 1 ? "" : "s"}`;
  switch (operation) {
    case WorkQueueItemBulkOperation.PATCH_PAYLOAD:
      return {
        title: "Confirm Payload Update",
        actionPhrase: `apply a payload merge patch to ${matchedCount}`,
        remainder: itemLabel,
        confirmLabel: "Apply Patch",
      };
    case WorkQueueItemBulkOperation.REPRIORITIZE:
      return {
        title: "Confirm Priority Change",
        actionPhrase: `set ${matchedCount}`,
        remainder: `${itemLabel} to`,
        emphasizedRemainder: `priority ${priority ?? 0}`,
        confirmLabel: "Change Priority",
      };
    case WorkQueueItemBulkOperation.CANCEL:
    default:
      return {
        title: "Confirm Bulk Cancellation",
        actionPhrase: `cancel ${matchedCount}`,
        remainder: itemLabel,
        confirmLabel: "Cancel Items",
      };
  }
}

function buildSelectorVariables(rows: SelectorVariableRow[]): Record<string, JsonValue> {
  const vars: Record<string, JsonValue> = {};
  const seen = new Set<string>();

  for (const row of rows) {
    const name = row.name.trim();
    if (!name) {
      throw new Error("Selector variable names are required.");
    }
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
      throw new Error(
        `Selector variable "${name}" must start with a letter or underscore and contain only letters, numbers, or underscores.`,
      );
    }
    if (seen.has(name)) {
      throw new Error(`Selector variable "${name}" is defined more than once.`);
    }
    seen.add(name);

    switch (row.type) {
      case "number": {
        const value = Number(row.value);
        if (!Number.isFinite(value)) {
          throw new Error(`Selector variable "${name}" must be a valid number.`);
        }
        vars[name] = value;
        break;
      }
      case "boolean":
        vars[name] = row.value === "true";
        break;
      case "null":
        vars[name] = null;
        break;
      case "json":
        try {
          vars[name] = JSON.parse(row.value) as JsonValue;
        } catch {
          throw new Error(`Selector variable "${name}" must contain valid JSON.`);
        }
        break;
      case "string":
      default:
        vars[name] = row.value;
    }
  }

  return vars;
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
  const [showBulkOperationsModal, setShowBulkOperationsModal] = useState(false);
  const [selectorInputMode, setSelectorInputMode] =
    useState<SelectorInputMode>("condition");
  const [selectorPath, setSelectorPath] = useState("@.priority == 50");
  const [selectorVars, setSelectorVars] = useState<SelectorVariableRow[]>([]);
  const [bulkOperation, setBulkOperation] = useState<WorkQueueItemBulkOperation>(
    WorkQueueItemBulkOperation.CANCEL,
  );
  const [bulkPriority, setBulkPriority] = useState(0);
  const [bulkPayloadPatch, setBulkPayloadPatch] = useState("{}");
  const [selectorError, setSelectorError] = useState<string | null>(null);
  const [selectorPreview, setSelectorPreview] =
    useState<PreviewWorkQueueItemsResponse | null>(null);
  const [bulkResult, setBulkResult] =
    useState<ApplyWorkQueueItemsResponse | null>(null);
  const [pendingBulkConfirmation, setPendingBulkConfirmation] =
    useState<PendingBulkConfirmation | null>(null);
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
  const previewBySelector = usePreviewQueueItemsBySelector();
  const applyBySelector = useApplyQueueItemsBySelector();

  const items = itemsData?.items ?? [];
  const itemPagination = itemsData?.pagination;
  const itemTotal = itemPagination?.total_items ?? 0;
  const sourceBadge = queue ? getQueueSourceBadge(queue.is_adhoc) : null;
  const canUpdateQueues = hasPermission(user, "queues", "update");
  const canReadQueueItems = hasPermission(user, "queue_items", "read");
  const canUpdateQueueItems = hasPermission(user, "queue_items", "update");
  const canDeleteQueueItems = hasPermission(user, "queue_items", "delete");
  const canUpdateQueuesResolved = true;
  const queueFlagControlsDisabled =
    !queue ||
    updateQueue.isPending ||
    (canUpdateQueuesResolved && !canUpdateQueues);
  const canApplyBulkOperation =
    bulkOperation === WorkQueueItemBulkOperation.CANCEL
      ? canDeleteQueueItems
      : canUpdateQueueItems;

  const clearItemFilters = () => {
    setItemKeyFilter("");
    setEnqueueSourceFilter("");
    setStatusFilter("pending");
    setPage(1);
  };

  const closeBulkOperationsModal = () => {
    setPendingBulkConfirmation(null);
    setSelectorPreview(null);
    setBulkResult(null);
    setSelectorError(null);
    setShowBulkOperationsModal(false);
  };

  const buildSelectorRequest = () => {
    const input = selectorPath.trim();
    if (!input) {
      throw new Error(
        selectorInputMode === "condition"
          ? "Selector condition is required."
          : "Selector path is required.",
      );
    }
    const path = selectorInputMode === "condition" ? `$ ? (${input})` : input;

    return {
      selector: {
        path,
        vars: buildSelectorVariables(selectorVars),
      },
    };
  };

  const updateSelectorVariable = (
    id: string,
    patch: Partial<Omit<SelectorVariableRow, "id">>,
  ) => {
    setSelectorVars((rows) =>
      rows.map((row) => {
        if (row.id !== id) {
          return row;
        }
        const next = { ...row, ...patch };
        if (patch.type === "boolean" && row.type !== "boolean") {
          next.value = "true";
        }
        if (patch.type === "null") {
          next.value = "";
        }
        return next;
      }),
    );
  };

  const removeSelectorVariable = (id: string) => {
    setSelectorVars((rows) => rows.filter((row) => row.id !== id));
  };

  const handlePreviewSelector = async () => {
    try {
      setSelectorError(null);
      setBulkResult(null);
      const base = buildSelectorRequest();
      const response = await previewBySelector.mutateAsync({
        ref: queueRef,
        data: {
          ...base,
          limit: 100,
        },
      });
      setSelectorPreview(response.data);
    } catch (previewError) {
      setSelectorError(
        getErrorMessage(previewError, "Failed to preview selected queue items"),
      );
    }
  };

  const buildBulkApplyRequest = () => {
    const base = buildSelectorRequest();
    switch (bulkOperation) {
      case WorkQueueItemBulkOperation.PATCH_PAYLOAD:
        return {
          ...base,
          operation: bulkOperation,
          payload_patch: parseJsonObject("Payload patch", bulkPayloadPatch),
          preview_limit: 100,
        };
      case WorkQueueItemBulkOperation.REPRIORITIZE:
        return {
          ...base,
          operation: bulkOperation,
          priority: bulkPriority,
          preview_limit: 100,
        };
      case WorkQueueItemBulkOperation.CANCEL:
      default:
        return {
          ...base,
          operation: bulkOperation,
          preview_limit: 100,
        };
    }
  };

  const handleApplySelector = async () => {
    try {
      setSelectorError(null);
      setPendingBulkConfirmation({
        request: buildBulkApplyRequest(),
        matchedCount: selectorPreview?.matched_count ?? 0,
      });
    } catch (applyError) {
      setSelectorError(
        getErrorMessage(applyError, "Failed to prepare bulk queue item operation"),
      );
    }
  };

  const handleConfirmBulkApply = async () => {
    if (!pendingBulkConfirmation) {
      return;
    }
    try {
      setSelectorError(null);
      const response = await applyBySelector.mutateAsync({
        ref: queueRef,
        data: pendingBulkConfirmation.request,
      });
      setBulkResult(response.data);
      setSelectorPreview({
        matched_count: response.data.matched_count,
        preview_count: response.data.preview_count,
        items: response.data.items,
      });
      setPendingBulkConfirmation(null);
      setActionError(null);
    } catch (applyError) {
      setPendingBulkConfirmation(null);
      setSelectorError(
        getErrorMessage(applyError, "Failed to apply bulk queue item operation"),
      );
    }
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

  const pendingBulkCopy = pendingBulkConfirmation
    ? getBulkConfirmationCopy(
        pendingBulkConfirmation.request.operation,
        pendingBulkConfirmation.matchedCount,
        pendingBulkConfirmation.request.priority,
      )
    : null;

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
          <div className="flex flex-wrap items-center justify-end gap-3">
            <div className="text-sm text-gray-600">
              {itemTotal > 0
                ? `${itemTotal} total item${itemTotal === 1 ? "" : "s"}`
                : "No items yet"}
              {isItemsFetching && !isItemsLoading ? " • refreshing…" : ""}
            </div>
            <button
              type="button"
              onClick={() => {
                setSelectorError(null);
                setBulkResult(null);
                setShowBulkOperationsModal(true);
              }}
              disabled={!canReadQueueItems}
              className="inline-flex items-center gap-2 rounded-lg border border-blue-200 bg-blue-50 px-3 py-2 text-sm font-medium text-blue-700 hover:bg-blue-100 disabled:cursor-not-allowed disabled:border-gray-200 disabled:bg-gray-100 disabled:text-gray-400"
              title={
                canReadQueueItems
                  ? "Select pending items with SQL/JSONPath"
                  : "Requires queue_items:read"
              }
            >
              <Filter className="h-4 w-4" />
              Select / bulk update
            </button>
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

      {showBulkOperationsModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
          <div className="flex max-h-[92vh] w-full max-w-6xl flex-col overflow-hidden rounded-lg bg-white shadow-xl">
            <div className="flex items-start justify-between gap-4 border-b border-gray-200 p-6">
              <div>
                <h2 className="text-2xl font-bold text-gray-900">
                  Select queue items
                </h2>
                <p className="mt-1 max-w-3xl text-sm text-gray-500">
                  Use PostgreSQL SQL/JSONPath to select unprocessed queued or
                  retry items in <span className="font-mono">{queue.ref}</span>.
                  Preview shows up to 100 matches before applying a bulk action.
                </p>
              </div>
              <button
                type="button"
                onClick={closeBulkOperationsModal}
                className="text-gray-400 hover:text-gray-600"
                aria-label="Close bulk queue item selector"
              >
                <X className="h-6 w-6" />
              </button>
            </div>

            <div className="flex-1 space-y-5 overflow-y-auto p-6">
              {selectorError && (
                <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
                  {selectorError}
                </div>
              )}
              {bulkResult && (
                <div className="rounded-lg border border-green-200 bg-green-50 px-4 py-3 text-sm text-green-800">
                  Applied {bulkResult.operation.replace("_", " ")} to{" "}
                  {bulkResult.affected_count} item
                  {bulkResult.affected_count === 1 ? "" : "s"}.
                  {bulkResult.skipped_count > 0
                    ? ` ${bulkResult.skipped_count} matching item${bulkResult.skipped_count === 1 ? "" : "s"} changed state before the update and were skipped.`
                    : ""}
                </div>
              )}

              <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_360px]">
                <div className="space-y-5">
                  <div>
                    <div className="mb-2 flex flex-wrap items-center justify-between gap-3">
                      <label className="block text-sm font-medium text-gray-700">
                        {selectorInputMode === "condition"
                          ? "Selector condition"
                          : "Raw SQL/JSONPath selector"}
                      </label>
                      <div className="inline-flex rounded-lg border border-gray-200 bg-gray-100 p-1 text-xs">
                        <button
                          type="button"
                          onClick={() => {
                            setSelectorInputMode("condition");
                            setSelectorPreview(null);
                            setBulkResult(null);
                          }}
                          className={`rounded-md px-2 py-1 ${
                            selectorInputMode === "condition"
                              ? "bg-white font-medium text-gray-900 shadow-sm"
                              : "text-gray-600 hover:text-gray-900"
                          }`}
                        >
                          Condition
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setSelectorInputMode("raw");
                            setSelectorPreview(null);
                            setBulkResult(null);
                          }}
                          className={`rounded-md px-2 py-1 ${
                            selectorInputMode === "raw"
                              ? "bg-white font-medium text-gray-900 shadow-sm"
                              : "text-gray-600 hover:text-gray-900"
                          }`}
                        >
                          Raw JSONPath
                        </button>
                      </div>
                    </div>
                    <input
                      value={selectorPath}
                      onChange={(event) => {
                        setSelectorPath(event.target.value);
                        setSelectorPreview(null);
                        setBulkResult(null);
                      }}
                      className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                      placeholder={
                        selectorInputMode === "condition"
                          ? "@.priority == 50 && @.payload.customer == $customer"
                          : '$.payload.tags[*] ? (@ == "urgent")'
                      }
                    />
                    {selectorInputMode === "condition" ? (
                      <p className="mt-1 text-xs text-gray-500">
                        Enter only the predicate expression. Attune sends it as{" "}
                        <span className="font-mono">$ ? (your condition)</span>.
                      </p>
                    ) : (
                      <p className="mt-1 text-xs text-gray-500">
                        Advanced mode sends the full PostgreSQL SQL/JSONPath
                        expression exactly as entered.
                      </p>
                    )}
                    <p className="mt-1 text-xs text-gray-500">
                      The selector root contains{" "}
                      <span className="font-mono">payload</span>,{" "}
                      <span className="font-mono">metadata</span>,{" "}
                      <span className="font-mono">item_key</span>,{" "}
                      <span className="font-mono">priority</span>,{" "}
                      <span className="font-mono">status</span>,{" "}
                      <span className="font-mono">enqueue_source</span>, and{" "}
                      <span className="font-mono">attempt_count</span>.
                    </p>
                    <div className="mt-3 rounded-lg border border-blue-100 bg-blue-50 p-3 text-xs text-blue-900">
                      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                        <span className="font-semibold">Examples</span>
                        <a
                          href="https://www.postgresql.org/docs/current/functions-json.html#FUNCTIONS-SQLJSON-PATH"
                          target="_blank"
                          rel="noreferrer"
                          className="inline-flex items-center gap-1 font-medium text-blue-700 hover:text-blue-900"
                        >
                          Full SQL/JSONPath syntax
                          <ExternalLink className="h-3 w-3" />
                        </a>
                      </div>
                      <div className="space-y-1 font-mono text-[11px] text-blue-950">
                        {selectorInputMode === "condition" ? (
                          <>
                            <div>
                              @.priority == 50
                              <span className="ml-2 font-sans text-blue-700">
                                priority equals 50
                              </span>
                            </div>
                            <div>
                              @.priority == $priority &amp;&amp;
                              @.payload.customer == $customer
                              <span className="ml-2 font-sans text-blue-700">
                                multiple fields with variables
                              </span>
                            </div>
                            <div>
                              @.metadata.source == "manual"
                              <span className="ml-2 font-sans text-blue-700">
                                metadata field match
                              </span>
                            </div>
                          </>
                        ) : (
                          <>
                            <div>
                              $.priority ? (@ == 50)
                              <span className="ml-2 font-sans text-blue-700">
                                priority equals 50
                              </span>
                            </div>
                            <div>
                              $ ? (@.priority == $priority &amp;&amp;
                              @.payload.customer == $customer)
                              <span className="ml-2 font-sans text-blue-700">
                                multiple fields with variables
                              </span>
                            </div>
                            <div>
                              $.metadata.source ? (@ == "manual")
                              <span className="ml-2 font-sans text-blue-700">
                                metadata field match
                              </span>
                            </div>
                          </>
                        )}
                      </div>
                    </div>
                  </div>

                  <div className="rounded-lg border border-gray-200 bg-gray-50 p-4">
                    <div className="mb-3 flex items-center justify-between gap-3">
                      <div>
                        <h3 className="text-sm font-semibold text-gray-900">
                          Selector variables
                        </h3>
                        <p className="mt-1 text-xs text-gray-500">
                          Add named variables referenced as{" "}
                          <span className="font-mono">$name</span> in the
                          selector. Names and values are validated before the
                          query runs.
                        </p>
                      </div>
                      <button
                        type="button"
                        onClick={() =>
                          setSelectorVars((rows) => [
                            ...rows,
                            createSelectorVariableRow(),
                          ])
                        }
                        className="shrink-0 rounded-lg bg-white px-3 py-2 text-sm font-medium text-gray-700 shadow hover:bg-gray-50"
                      >
                        Add variable
                      </button>
                    </div>

                    {selectorVars.length === 0 ? (
                      <div className="rounded-lg border border-dashed border-gray-300 bg-white px-4 py-5 text-center text-sm text-gray-500">
                        No selector variables. Add one if your JSONPath uses{" "}
                        <span className="font-mono">$variable</span> bindings.
                      </div>
                    ) : (
                      <div className="space-y-3">
                        {selectorVars.map((variable) => (
                          <div
                            key={variable.id}
                            className="grid gap-3 rounded-lg border border-gray-200 bg-white p-3 md:grid-cols-[180px_140px_minmax(0,1fr)_auto]"
                          >
                            <input
                              value={variable.name}
                              onChange={(event) => {
                                updateSelectorVariable(variable.id, {
                                  name: event.target.value,
                                });
                                setSelectorPreview(null);
                                setBulkResult(null);
                              }}
                              className="rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                              placeholder="customer_id"
                              aria-label="Variable name"
                            />
                            <select
                              value={variable.type}
                              onChange={(event) => {
                                updateSelectorVariable(variable.id, {
                                  type: event.target.value as SelectorVariableType,
                                });
                                setSelectorPreview(null);
                                setBulkResult(null);
                              }}
                              className="rounded-lg border border-gray-300 px-3 py-2 text-sm"
                              aria-label="Variable type"
                            >
                              <option value="string">String</option>
                              <option value="number">Number</option>
                              <option value="boolean">Boolean</option>
                              <option value="null">Null</option>
                              <option value="json">JSON</option>
                            </select>
                            {variable.type === "boolean" ? (
                              <select
                                value={variable.value || "true"}
                                onChange={(event) => {
                                  updateSelectorVariable(variable.id, {
                                    value: event.target.value,
                                  });
                                  setSelectorPreview(null);
                                  setBulkResult(null);
                                }}
                                className="rounded-lg border border-gray-300 px-3 py-2 text-sm"
                                aria-label="Variable value"
                              >
                                <option value="true">true</option>
                                <option value="false">false</option>
                              </select>
                            ) : variable.type === "json" ? (
                              <textarea
                                value={variable.value}
                                onChange={(event) => {
                                  updateSelectorVariable(variable.id, {
                                    value: event.target.value,
                                  });
                                  setSelectorPreview(null);
                                  setBulkResult(null);
                                }}
                                rows={2}
                                className="rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                                placeholder='{"tier":"gold"}'
                                aria-label="Variable JSON value"
                              />
                            ) : (
                              <input
                                value={
                                  variable.type === "null" ? "null" : variable.value
                                }
                                onChange={(event) => {
                                  updateSelectorVariable(variable.id, {
                                    value: event.target.value,
                                  });
                                  setSelectorPreview(null);
                                  setBulkResult(null);
                                }}
                                disabled={variable.type === "null"}
                                className="rounded-lg border border-gray-300 px-3 py-2 text-sm disabled:bg-gray-100"
                                placeholder={
                                  variable.type === "number" ? "123" : "value"
                                }
                                aria-label="Variable value"
                              />
                            )}
                            <button
                              type="button"
                              onClick={() => {
                                removeSelectorVariable(variable.id);
                                setSelectorPreview(null);
                                setBulkResult(null);
                              }}
                              className="self-start rounded-lg px-2 py-2 text-gray-400 hover:bg-gray-100 hover:text-red-600"
                              aria-label="Remove selector variable"
                            >
                              <Trash2 className="h-4 w-4" />
                            </button>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                </div>

                <div className="space-y-4 rounded-lg border border-gray-200 bg-gray-50 p-4">
                  <div>
                    <label className="mb-1 block text-sm font-medium text-gray-700">
                      Bulk operation
                    </label>
                    <select
                      value={bulkOperation}
                      onChange={(event) => {
                        setBulkOperation(
                          event.target.value as WorkQueueItemBulkOperation,
                        );
                        setBulkResult(null);
                      }}
                      className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                    >
                      <option value={WorkQueueItemBulkOperation.CANCEL}>
                        Cancel matching items
                      </option>
                      <option value={WorkQueueItemBulkOperation.PATCH_PAYLOAD}>
                        Merge-patch payload
                      </option>
                      <option value={WorkQueueItemBulkOperation.REPRIORITIZE}>
                        Re-prioritize
                      </option>
                    </select>
                  </div>

                  {bulkOperation === WorkQueueItemBulkOperation.REPRIORITIZE && (
                    <div>
                      <label className="mb-1 block text-sm font-medium text-gray-700">
                        New priority
                      </label>
                      <input
                        type="number"
                        value={bulkPriority}
                        onChange={(event) =>
                          setBulkPriority(Number(event.target.value))
                        }
                        className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                      />
                    </div>
                  )}

                  {bulkOperation === WorkQueueItemBulkOperation.PATCH_PAYLOAD && (
                    <div>
                      <label className="mb-1 block text-sm font-medium text-gray-700">
                        Payload merge patch
                      </label>
                      <textarea
                        value={bulkPayloadPatch}
                        onChange={(event) =>
                          setBulkPayloadPatch(event.target.value)
                        }
                        rows={5}
                        className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                      />
                      <p className="mt-1 text-xs text-gray-500">
                        RFC 7396-style object patch. Null values remove payload keys.
                      </p>
                    </div>
                  )}

                  <div className="flex flex-wrap gap-2">
                    <button
                      type="button"
                      onClick={handlePreviewSelector}
                      disabled={!canReadQueueItems || previewBySelector.isPending}
                      className="inline-flex items-center rounded-lg bg-white px-3 py-2 text-sm font-medium text-gray-700 shadow hover:bg-gray-50 disabled:cursor-not-allowed disabled:bg-gray-100 disabled:text-gray-400"
                    >
                      {previewBySelector.isPending ? "Previewing..." : "Preview matches"}
                    </button>
                    <button
                      type="button"
                      onClick={handleApplySelector}
                      disabled={
                        !canApplyBulkOperation ||
                        !selectorPreview ||
                        applyBySelector.isPending
                      }
                      className="inline-flex items-center rounded-lg bg-red-600 px-3 py-2 text-sm font-medium text-white hover:bg-red-700 disabled:cursor-not-allowed disabled:bg-gray-300"
                    >
                      {applyBySelector.isPending
                        ? "Applying..."
                        : "Apply bulk operation"}
                    </button>
                  </div>
                  {!selectorPreview && (
                    <p className="text-xs text-gray-500">
                      Preview matches before applying a bulk operation.
                    </p>
                  )}
                  {!canApplyBulkOperation && (
                    <p className="text-xs text-gray-500">
                      Applying this operation requires{" "}
                      <span className="font-mono">
                        {bulkOperation === WorkQueueItemBulkOperation.CANCEL
                          ? "queue_items:delete"
                          : "queue_items:update"}
                      </span>.
                    </p>
                  )}
                </div>
              </div>

              {selectorPreview && (
                <div>
                  <div className="mb-3 text-sm text-gray-600">
                    {selectorPreview.matched_count} unprocessed item
                    {selectorPreview.matched_count === 1 ? "" : "s"} match.
                    Showing {selectorPreview.preview_count} preview item
                    {selectorPreview.preview_count === 1 ? "" : "s"}.
                  </div>
                  {selectorPreview.items.length > 0 ? (
                    <div className="max-h-80 overflow-auto rounded-lg border border-gray-200">
                      <table className="min-w-full divide-y divide-gray-200">
                        <thead className="sticky top-0 bg-gray-50">
                          <tr>
                            <th className="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                              ID / key
                            </th>
                            <th className="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                              Status
                            </th>
                            <th className="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                              Priority
                            </th>
                            <th className="px-3 py-2 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                              Payload
                            </th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-gray-200 bg-white">
                          {selectorPreview.items.map((item) => {
                            const statusBadge = getStatusBadge(item.status);
                            return (
                              <tr key={item.id}>
                                <td className="px-3 py-3 align-top text-sm">
                                  <div className="font-semibold text-gray-900">
                                    #{item.id}
                                  </div>
                                  <div className="mt-1 font-mono text-xs text-gray-500">
                                    {item.item_key || "—"}
                                  </div>
                                </td>
                                <td className="px-3 py-3 align-top">
                                  <span
                                    className={`inline-flex rounded-full px-2 py-1 text-xs font-semibold ${statusBadge.classes}`}
                                  >
                                    {statusBadge.label}
                                  </span>
                                </td>
                                <td className="px-3 py-3 align-top text-sm text-gray-700">
                                  {item.priority}
                                </td>
                                <td className="px-3 py-3 align-top">
                                  <pre className="max-w-xl whitespace-pre-wrap break-words text-xs text-gray-700">
                                    {formatJsonPreview(item.payload, 200)}
                                  </pre>
                                </td>
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  ) : (
                    <div className="rounded-lg border border-dashed border-gray-300 px-4 py-6 text-center text-sm text-gray-500">
                      No unprocessed queue items match this selector.
                    </div>
                  )}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {showBulkOperationsModal && pendingBulkConfirmation && pendingBulkCopy && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 p-4">
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="bulk-confirm-title"
            className="w-full max-w-lg rounded-lg bg-white shadow-xl"
          >
            <div className="flex items-start gap-3 border-b border-gray-200 p-5">
              <div className="rounded-full bg-amber-100 p-2 text-amber-700">
                <AlertTriangle className="h-5 w-5" />
              </div>
              <div>
                <h3
                  id="bulk-confirm-title"
                  className="text-lg font-semibold text-gray-900"
                >
                  {pendingBulkCopy.title}
                </h3>
                <p className="mt-1 text-sm text-gray-600">
                  You are about to{" "}
                  <span className="font-semibold">
                    {pendingBulkCopy.actionPhrase}
                  </span>{" "}
                  {pendingBulkCopy.remainder}
                  {pendingBulkCopy.emphasizedRemainder ? (
                    <>
                      {" "}
                      <span className="font-semibold">
                        {pendingBulkCopy.emphasizedRemainder}
                      </span>
                    </>
                  ) : null}
                  .
                </p>
              </div>
            </div>
            <div className="space-y-3 p-5">
              <p className="text-sm text-gray-500">
                Only items that are still queued or retrying when the update runs
                will be changed.
              </p>
              <div className="flex justify-end gap-3">
                <button
                  type="button"
                  onClick={() => setPendingBulkConfirmation(null)}
                  disabled={applyBySelector.isPending}
                  className="rounded-lg border border-gray-300 px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:cursor-not-allowed disabled:bg-gray-100 disabled:text-gray-400"
                >
                  Back
                </button>
                <button
                  type="button"
                  onClick={handleConfirmBulkApply}
                  disabled={applyBySelector.isPending}
                  className="rounded-lg bg-red-600 px-4 py-2 text-sm font-medium text-white hover:bg-red-700 disabled:cursor-not-allowed disabled:bg-gray-300"
                >
                  {applyBySelector.isPending
                    ? "Applying..."
                    : pendingBulkCopy.confirmLabel}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

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
