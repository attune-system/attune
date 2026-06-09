import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useActions } from "@/hooks/useActions";
import { useCreateQueue, useUpdateQueue } from "@/hooks/useQueues";
import {
  WorkQueueBatchMode,
  WorkQueueUpdateStrategy,
  ReferenceVisibility,
  type JsonValue,
  type WorkQueueResponse,
} from "@/api/queues";
import {
  parseQueueConfig,
  parseJsonObject,
  prettyJson,
} from "./queueUtils";

interface QueueFormProps {
  initialData?: WorkQueueResponse;
  isEditing?: boolean;
}

type PermissionOverrideMode = "inherit" | "none" | "custom";

function getErrorMessage(error: unknown, fallback: string): string {
  const maybeAxios = error as { response?: { data?: { message?: string } } };
  return maybeAxios.response?.data?.message ||
    (error instanceof Error ? error.message : fallback);
}

export default function QueueForm({
  initialData,
  isEditing = false,
}: QueueFormProps) {
  const navigate = useNavigate();
  const createQueue = useCreateQueue();
  const updateQueue = useUpdateQueue();
  const { data: actionsData } = useActions({ page: 1, pageSize: 200 });

  const [ref, setRef] = useState(() => initialData?.ref ?? "");
  const [label, setLabel] = useState(() => initialData?.label ?? "");
  const [description, setDescription] = useState(
    () => initialData?.description ?? "",
  );
  const [dispatchActionRef, setDispatchActionRef] = useState(
    () => initialData?.dispatch_action_ref ?? "",
  );
  const [enabled, setEnabled] = useState(() => initialData?.enabled ?? true);
  const [acceptingNewItems, setAcceptingNewItems] = useState(
    () => initialData?.accepting_new_items ?? true,
  );
  const [referenceVisibility, setReferenceVisibility] =
    useState<ReferenceVisibility>(
      () => initialData?.reference_visibility ?? ReferenceVisibility.PUBLIC,
    );
  const [referenceAllowedPackRefs, setReferenceAllowedPackRefs] = useState(
    () => (initialData?.reference_allowed_pack_refs ?? []).join("\n"),
  );
  const [defaultPriority, setDefaultPriority] = useState(
    () => initialData?.default_priority ?? 0,
  );
  const [allowPendingUpdate, setAllowPendingUpdate] = useState(
    () => initialData?.allow_pending_update ?? false,
  );
  const [updateStrategy, setUpdateStrategy] = useState<WorkQueueUpdateStrategy>(
    () => initialData?.update_strategy ?? WorkQueueUpdateStrategy.REPLACE,
  );
  const [batchMode, setBatchMode] = useState<WorkQueueBatchMode>(
    () => initialData?.batch_mode ?? WorkQueueBatchMode.SINGLE,
  );
  const [itemSchema, setItemSchema] = useState(
    () => prettyJson(initialData?.item_schema),
  );
  const [actionParams, setActionParams] = useState(
    () => prettyJson(initialData?.action_params),
  );
  const initialPermissionSetRefs = initialData?.permission_set_refs ?? null;
  const [permissionMode, setPermissionMode] = useState<PermissionOverrideMode>(
    initialPermissionSetRefs === null
      ? "inherit"
      : initialPermissionSetRefs.length === 0
        ? "none"
        : "custom",
  );
  const [permissionSetRefsInput, setPermissionSetRefsInput] = useState(
    () => initialPermissionSetRefs?.join(", ") ?? "",
  );
  const [config, setConfig] = useState(() => prettyJson(initialData?.config));
  const initialQueueConfig = parseQueueConfig(initialData?.config);
  const [coalescingEnabled, setCoalescingEnabled] = useState(
    () => initialQueueConfig.dispatch?.coalescing?.enabled ?? false,
  );
  const [interExecutionDelaySeconds, setInterExecutionDelaySeconds] = useState(
    () =>
      initialQueueConfig.dispatch?.inter_execution_delay_seconds !== undefined
        ? String(initialQueueConfig.dispatch.inter_execution_delay_seconds)
        : "",
  );
  const [retryLimit, setRetryLimit] = useState(
    () =>
      initialQueueConfig.dispatch?.retry_limit !== undefined
        ? String(initialQueueConfig.dispatch.retry_limit)
        : "",
  );
  const [coalescingGroupByPath, setCoalescingGroupByPath] = useState(
    () => initialQueueConfig.dispatch?.coalescing?.group_by_path ?? "",
  );
  const [coalescingAcrossPriorities, setCoalescingAcrossPriorities] = useState(
    () => initialQueueConfig.dispatch?.coalescing?.across_priorities ?? false,
  );
  const [errors, setErrors] = useState<Record<string, string>>({});
  const isImmutableStrategy = updateStrategy === WorkQueueUpdateStrategy.IMMUTABLE;
  const effectiveAllowPendingUpdate = isImmutableStrategy ? false : allowPendingUpdate;

  let parsedDispatchConfig: ReturnType<typeof parseQueueConfig> | null = null;
  try {
    parsedDispatchConfig = parseQueueConfig(parseJsonObject("Config", config));
  } catch {
    // Keep the current control state while the user edits invalid JSON.
  }

  const effectiveInterExecutionDelaySeconds =
    parsedDispatchConfig?.dispatch?.inter_execution_delay_seconds !== undefined
      ? String(parsedDispatchConfig.dispatch.inter_execution_delay_seconds)
      : interExecutionDelaySeconds;
  const effectiveRetryLimit =
    parsedDispatchConfig?.dispatch?.retry_limit !== undefined
      ? String(parsedDispatchConfig.dispatch.retry_limit)
      : retryLimit;
  const effectiveCoalescingEnabled =
    parsedDispatchConfig?.dispatch?.coalescing?.enabled ?? coalescingEnabled;
  const effectiveCoalescingGroupByPath =
    parsedDispatchConfig?.dispatch?.coalescing?.group_by_path ?? coalescingGroupByPath;
  const effectiveCoalescingAcrossPriorities =
    parsedDispatchConfig?.dispatch?.coalescing?.across_priorities ??
    coalescingAcrossPriorities;

  const applyDispatchSettingsToConfig = (
    configObject: ReturnType<typeof parseJsonObject>,
    nextRetryLimit: string,
    nextDelaySeconds: string,
    nextEnabled: boolean,
    nextGroupByPath: string,
    nextAcrossPriorities: boolean,
  ) => {
    const nextConfig = { ...configObject };
    const currentDispatch = nextConfig.dispatch;
    const dispatch: Record<string, JsonValue> =
      currentDispatch && typeof currentDispatch === "object" && !Array.isArray(currentDispatch)
        ? { ...(currentDispatch as Record<string, JsonValue>) }
        : {};

    const trimmedRetryLimit = nextRetryLimit.trim();
    if (!trimmedRetryLimit) {
      delete dispatch.retry_limit;
    } else {
      const parsedRetryLimit = Number(trimmedRetryLimit);
      if (!Number.isInteger(parsedRetryLimit) || parsedRetryLimit < 0) {
        throw new Error("Retry limit must be a non-negative integer");
      }
      dispatch.retry_limit = parsedRetryLimit;
    }

    const trimmedDelay = nextDelaySeconds.trim();
    if (!trimmedDelay) {
      delete dispatch.inter_execution_delay_seconds;
    } else {
      const parsedDelay = Number(trimmedDelay);
      if (!Number.isInteger(parsedDelay) || parsedDelay < 0) {
        throw new Error("Sequential inter-execution delay must be a non-negative integer");
      }
      dispatch.inter_execution_delay_seconds = parsedDelay;
    }

    if (
      batchMode !== WorkQueueBatchMode.BATCH ||
      (!nextEnabled && !nextGroupByPath.trim() && !nextAcrossPriorities)
    ) {
      delete dispatch.coalescing;
    } else {
      const nextCoalescing: Record<string, JsonValue> = {
        enabled: nextEnabled,
        across_priorities: nextAcrossPriorities,
      };
      if (nextGroupByPath.trim()) {
        nextCoalescing.group_by_path = nextGroupByPath.trim();
      }
      dispatch.coalescing = nextCoalescing;
    }

    if (Object.keys(dispatch).length === 0) {
      delete nextConfig.dispatch;
    } else {
      nextConfig.dispatch = dispatch;
    }

    return nextConfig;
  };

  const updateDispatchConfig = (
    nextRetryLimit: string,
    nextDelaySeconds: string,
    nextEnabled: boolean,
    nextGroupByPath: string,
    nextAcrossPriorities: boolean,
  ) => {
    let parsedConfig: ReturnType<typeof parseJsonObject>;
    try {
      parsedConfig = parseJsonObject("Config", config);
    } catch {
      parsedConfig = {};
    }

    setConfig(
      prettyJson(
        applyDispatchSettingsToConfig(
          parsedConfig,
          nextRetryLimit,
          nextDelaySeconds,
          nextEnabled,
          nextGroupByPath,
          nextAcrossPriorities,
        ),
      ),
    );
  };

  const actions = actionsData?.items ?? [];
  const actionOptions =
    initialData?.dispatch_action_ref &&
    !actions.some((action) => action.ref === initialData.dispatch_action_ref)
      ? [
          {
            id: -1,
            ref: initialData.dispatch_action_ref,
            label: initialData.dispatch_action_ref,
          },
          ...actions,
        ]
      : actions;

  const isSubmitting = createQueue.isPending || updateQueue.isPending;

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();

    const nextErrors: Record<string, string> = {};
    if (!ref.trim() && !isEditing) {
      nextErrors.ref = "Queue ref is required";
    }
    if (!label.trim()) {
      nextErrors.label = "Label is required";
    }
    if (!dispatchActionRef.trim()) {
      nextErrors.dispatch_action_ref = "Dispatch action is required";
    }
    const allowedPackRefs = referenceAllowedPackRefs
      .split(/\r?\n|,/)
      .map((packRef) => packRef.trim())
      .filter(Boolean);
    if (
      referenceVisibility !== ReferenceVisibility.RESTRICTED &&
      allowedPackRefs.length > 0
    ) {
      nextErrors.reference_allowed_pack_refs =
        "Allowed pack refs can only be set when visibility is restricted";
    }

    let parsedItemSchema: ReturnType<typeof parseJsonObject> | undefined;
    try {
      parsedItemSchema = parseJsonObject("Queue item schema", itemSchema);
    } catch (error) {
      nextErrors.item_schema =
        error instanceof Error ? error.message : "Queue item schema must be valid JSON";
    }

    let parsedActionParams: ReturnType<typeof parseJsonObject> | undefined;
    try {
      parsedActionParams = parseJsonObject("Action parameters", actionParams);
    } catch (error) {
      nextErrors.action_params =
        error instanceof Error ? error.message : "Action parameters must be valid JSON";
    }

    let parsedConfig: ReturnType<typeof parseJsonObject> | undefined;
    try {
      parsedConfig = applyDispatchSettingsToConfig(
        parseJsonObject("Config", config),
        effectiveRetryLimit,
        effectiveInterExecutionDelaySeconds,
        effectiveCoalescingEnabled,
        effectiveCoalescingGroupByPath,
        effectiveCoalescingAcrossPriorities,
      );
    } catch (error) {
      nextErrors.config =
        error instanceof Error ? error.message : "Config must be valid JSON";
    }

    if (effectiveRetryLimit.trim()) {
      const parsedRetryLimit = Number(effectiveRetryLimit.trim());
      if (!Number.isInteger(parsedRetryLimit) || parsedRetryLimit < 0) {
        nextErrors.retry_limit = "Retry limit must be a non-negative integer";
      }
    }

    if (effectiveInterExecutionDelaySeconds.trim()) {
      const parsedDelay = Number(effectiveInterExecutionDelaySeconds.trim());
      if (!Number.isInteger(parsedDelay) || parsedDelay < 0) {
        nextErrors.inter_execution_delay_seconds =
          "Sequential inter-execution delay must be a non-negative integer";
      }
    }

    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      return;
    }

    try {
      const permissionSetRefs =
        permissionMode === "custom"
          ? permissionSetRefsInput
              .split(",")
              .map((permissionSetRef) => permissionSetRef.trim())
              .filter(Boolean)
          : permissionMode === "none"
            ? []
            : null;

      if (isEditing && initialData) {
        await updateQueue.mutateAsync({
          ref: initialData.ref,
          data: {
            label: label.trim(),
            description: description.trim()
              ? { op: "set", value: description.trim() }
              : { op: "clear" },
            enabled,
            accepting_new_items: acceptingNewItems,
            dispatch_action_ref: dispatchActionRef,
            default_priority: defaultPriority,
            allow_pending_update: effectiveAllowPendingUpdate,
            update_strategy: updateStrategy,
            batch_mode: batchMode,
            item_schema: parsedItemSchema,
            action_params: parsedActionParams,
            permission_set_refs: permissionSetRefs,
            config: parsedConfig,
            reference_visibility: referenceVisibility,
            reference_allowed_pack_refs: allowedPackRefs,
          },
        });
        navigate(`/queues/${encodeURIComponent(initialData.ref)}`);
        return;
      }

      const response = await createQueue.mutateAsync({
        ref: ref.trim(),
        label: label.trim(),
        description: description.trim() || null,
        enabled,
        accepting_new_items: acceptingNewItems,
        dispatch_action_ref: dispatchActionRef,
        default_priority: defaultPriority,
        allow_pending_update: effectiveAllowPendingUpdate,
        update_strategy: updateStrategy,
        batch_mode: batchMode,
        item_schema: parsedItemSchema,
        action_params: parsedActionParams,
        ...(permissionSetRefs === null
          ? {}
          : { permission_set_refs: permissionSetRefs }),
        config: parsedConfig,
        reference_visibility: referenceVisibility,
        reference_allowed_pack_refs: allowedPackRefs,
      });
      navigate(`/queues/${encodeURIComponent(response.data.ref)}`);
    } catch (error) {
      setErrors({
        submit: getErrorMessage(error, "Failed to save queue"),
      });
    }
  };

  return (
    <form onSubmit={handleSubmit} className="space-y-6">
      {errors.submit && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700">
          {errors.submit}
        </div>
      )}

      <div className="grid gap-6 lg:grid-cols-2">
        <div className="bg-white rounded-lg shadow p-6 space-y-4">
          <h2 className="text-lg font-semibold text-gray-900">Basics</h2>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Queue Ref
            </label>
            <input
              value={ref}
              onChange={(e) => setRef(e.target.value)}
              disabled={isEditing}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm disabled:bg-gray-100"
              placeholder="ops.manual_review"
            />
            <p className="mt-1 text-xs text-gray-500">
              Use a unique ref in <span className="font-mono">pack.name</span> format.
            </p>
            {errors.ref && <p className="mt-1 text-sm text-red-600">{errors.ref}</p>}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Label
            </label>
            <input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Manual Review Queue"
            />
            {errors.label && <p className="mt-1 text-sm text-red-600">{errors.label}</p>}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Description
            </label>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={4}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Describe what this queue dispatches and who uses it"
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Dispatch Action
            </label>
            <select
              value={dispatchActionRef}
              onChange={(e) => setDispatchActionRef(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="">Select an action</option>
              {actionOptions.map((action) => (
                <option key={action.ref} value={action.ref}>
                  {action.ref}
                  {action.label && action.label !== action.ref
                    ? ` — ${action.label}`
                    : ""}
                </option>
              ))}
            </select>
            {errors.dispatch_action_ref && (
              <p className="mt-1 text-sm text-red-600">{errors.dispatch_action_ref}</p>
            )}
          </div>

          <label className="flex items-start gap-3 rounded-lg border border-gray-200 p-4">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => setEnabled(e.target.checked)}
              className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
            />
            <span>
              <span className="block text-sm font-medium text-gray-900">
                Executor processing enabled
              </span>
              <span className="block text-sm text-gray-500">
                Disabled queues remain visible but the executor will stop dispatching items from them.
              </span>
            </span>
          </label>

          <label className="flex items-start gap-3 rounded-lg border border-gray-200 p-4">
            <input
              type="checkbox"
              checked={acceptingNewItems}
              onChange={(e) => setAcceptingNewItems(e.target.checked)}
              className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
            />
            <span>
              <span className="block text-sm font-medium text-gray-900">
                Accept new items
              </span>
              <span className="block text-sm text-gray-500">
                Disable this to reject enqueue requests while keeping current items intact.
              </span>
            </span>
          </label>

          <div className="rounded-lg border border-gray-200 p-4 space-y-3">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Queue reference visibility
              </label>
              <select
                value={referenceVisibility}
                onChange={(e) =>
                  setReferenceVisibility(e.target.value as ReferenceVisibility)
                }
                className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              >
                <option value={ReferenceVisibility.PUBLIC}>
                  Public - any pack may submit items
                </option>
                <option value={ReferenceVisibility.PRIVATE}>
                  Private - only this queue&apos;s pack may submit items
                </option>
                <option value={ReferenceVisibility.RESTRICTED}>
                  Restricted - this pack plus allowed packs may submit items
                </option>
              </select>
              <p className="mt-1 text-xs text-gray-500">
                Visibility controls pack-to-queue targeting and item submission;
                queue-item RBAC still applies.
              </p>
            </div>

            {referenceVisibility === ReferenceVisibility.RESTRICTED && (
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">
                  Allowed pack refs
                </label>
                <textarea
                  value={referenceAllowedPackRefs}
                  onChange={(e) => setReferenceAllowedPackRefs(e.target.value)}
                  rows={4}
                  className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                  placeholder={"service_pack\nops_pack"}
                />
                <p className="mt-1 text-xs text-gray-500">
                  One pack ref per line, or comma-separated.
                </p>
              </div>
            )}
            {errors.reference_allowed_pack_refs && (
              <p className="text-sm text-red-600">
                {errors.reference_allowed_pack_refs}
              </p>
            )}
          </div>
        </div>

        <div className="bg-white rounded-lg shadow p-6 space-y-4">
          <h2 className="text-lg font-semibold text-gray-900">Dispatch behaviour</h2>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Default priority
            </label>
            <input
              type="number"
              value={defaultPriority}
              onChange={(e) => setDefaultPriority(Number(e.target.value))}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            />
          </div>

          <label className="flex items-start gap-3 rounded-lg border border-gray-200 p-4">
            <input
              type="checkbox"
              checked={effectiveAllowPendingUpdate}
              onChange={(e) => setAllowPendingUpdate(e.target.checked)}
              disabled={isImmutableStrategy}
              className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
            />
            <span>
              <span className="block text-sm font-medium text-gray-900">
                Allow pending item updates
              </span>
              <span className="block text-sm text-gray-500">
                When enabled, enqueue requests can update an existing queued or retry item with the same key.
              </span>
              {isImmutableStrategy && (
                <span className="mt-2 block text-sm text-amber-700">
                  Immutable queues always reject duplicate pending keys, so pending updates are turned off.
                </span>
              )}
            </span>
          </label>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Update strategy
            </label>
            <select
              value={updateStrategy}
              onChange={(e) =>
                setUpdateStrategy(e.target.value as WorkQueueUpdateStrategy)
              }
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value={WorkQueueUpdateStrategy.REPLACE}>Replace existing payload + metadata</option>
              <option value={WorkQueueUpdateStrategy.MERGE_PATCH}>Merge patch existing payload + metadata</option>
              <option value={WorkQueueUpdateStrategy.IMMUTABLE}>Reject duplicate pending item keys</option>
            </select>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Batch mode
            </label>
            <select
              value={batchMode}
              onChange={(e) => setBatchMode(e.target.value as WorkQueueBatchMode)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value={WorkQueueBatchMode.SINGLE}>Single item dispatch</option>
              <option value={WorkQueueBatchMode.BATCH}>Batch dispatch</option>
            </select>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Retry limit
            </label>
            <input
              type="number"
              min={0}
              step={1}
              value={effectiveRetryLimit}
              onChange={(e) => {
                const nextValue = e.target.value;
                setRetryLimit(nextValue);
                updateDispatchConfig(
                  nextValue,
                  effectiveInterExecutionDelaySeconds,
                  effectiveCoalescingEnabled,
                  effectiveCoalescingGroupByPath,
                  effectiveCoalescingAcrossPriorities,
                );
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="0"
            />
            <p className="mt-1 text-xs text-gray-500">
              Number of times an item may return to <span className="font-mono">Retry</span> before it is marked <span className="font-mono">Failed</span>.
            </p>
            {errors.retry_limit && (
              <p className="mt-1 text-sm text-red-600">{errors.retry_limit}</p>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Sequential inter-execution delay (seconds)
            </label>
            <input
              type="number"
              min={0}
              step={1}
              value={effectiveInterExecutionDelaySeconds}
              onChange={(e) => {
                const nextValue = e.target.value;
                setInterExecutionDelaySeconds(nextValue);
                updateDispatchConfig(
                  effectiveRetryLimit,
                  nextValue,
                  effectiveCoalescingEnabled,
                  effectiveCoalescingGroupByPath,
                  effectiveCoalescingAcrossPriorities,
                );
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="0"
            />
            <p className="mt-1 text-xs text-gray-500">
              Only applies when concurrency resolves to <span className="font-mono">1</span>. The cooldown starts after the prior queue execution reaches a terminal state.
            </p>
            {errors.inter_execution_delay_seconds && (
              <p className="mt-1 text-sm text-red-600">{errors.inter_execution_delay_seconds}</p>
            )}
          </div>

          <div className="rounded-lg border border-gray-200 p-4 space-y-4">
            <div>
              <h3 className="text-sm font-medium text-gray-900">Batch coalescing</h3>
              <p className="mt-1 text-xs text-gray-500">
                Start from the first queued batch item, then hoist later items with the same grouping value.
              </p>
            </div>

            <label className="flex items-start gap-3">
              <input
                type="checkbox"
                checked={effectiveCoalescingEnabled}
                disabled={batchMode !== WorkQueueBatchMode.BATCH}
                onChange={(e) => {
                  const nextEnabled = e.target.checked;
                  setCoalescingEnabled(nextEnabled);
                  updateDispatchConfig(
                    effectiveRetryLimit,
                    effectiveInterExecutionDelaySeconds,
                    nextEnabled,
                    effectiveCoalescingGroupByPath,
                    effectiveCoalescingAcrossPriorities,
                  );
                }}
                className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500 disabled:cursor-not-allowed"
              />
              <span>
                <span className="block text-sm font-medium text-gray-900">
                  Enable coalescing
                </span>
                <span className="block text-sm text-gray-500">
                  Available only for batch queues.
                </span>
              </span>
            </label>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Group by payload path
              </label>
              <input
                value={effectiveCoalescingGroupByPath}
                disabled={batchMode !== WorkQueueBatchMode.BATCH || !effectiveCoalescingEnabled}
                onChange={(e) => {
                  const nextValue = e.target.value;
                  setCoalescingGroupByPath(nextValue);
                  updateDispatchConfig(
                    effectiveRetryLimit,
                    effectiveInterExecutionDelaySeconds,
                    effectiveCoalescingEnabled,
                    nextValue,
                    effectiveCoalescingAcrossPriorities,
                  );
                }}
                className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm disabled:bg-gray-100"
                placeholder="attributes.sobject_type"
              />
              <p className="mt-1 text-xs text-gray-500">
                Dot-separated payload path used to group batch items, such as
                <span className="font-mono"> attributes.sobject_type</span>.
              </p>
            </div>

            <label className="flex items-start gap-3">
              <input
                type="checkbox"
                checked={effectiveCoalescingAcrossPriorities}
                disabled={batchMode !== WorkQueueBatchMode.BATCH || !effectiveCoalescingEnabled}
                onChange={(e) => {
                  const nextValue = e.target.checked;
                  setCoalescingAcrossPriorities(nextValue);
                  updateDispatchConfig(
                    effectiveRetryLimit,
                    effectiveInterExecutionDelaySeconds,
                    effectiveCoalescingEnabled,
                    effectiveCoalescingGroupByPath,
                    nextValue,
                  );
                }}
                className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500 disabled:cursor-not-allowed"
              />
              <span>
                <span className="block text-sm font-medium text-gray-900">
                  Coalesce batches across priorities
                </span>
                <span className="block text-sm text-gray-500">
                  When disabled, only items in the anchor item&apos;s priority band may be hoisted into the batch.
                </span>
              </span>
            </label>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Queue item schema (JSON)
            </label>
            <textarea
              value={itemSchema}
              onChange={(e) => setItemSchema(e.target.value)}
              rows={10}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
              placeholder={'{\n  "order_id": { "type": "integer", "required": true }\n}'}
            />
            <p className="mt-1 text-xs text-gray-500">
              Uses the same flat schema format as triggers and is enforced when queue items are enqueued or updated.
            </p>
            {errors.item_schema && (
              <p className="mt-1 text-sm text-red-600">{errors.item_schema}</p>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Action parameters (JSON)
            </label>
            <textarea
              value={actionParams}
              onChange={(e) => setActionParams(e.target.value)}
              rows={10}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
              placeholder={'{\n  "item": "{{ item }}"\n}'}
            />
            <p className="mt-1 text-xs text-gray-500">
              Use queue templates like <span className="font-mono">{"{{ item }}"}</span> for single dispatch,
              <span className="font-mono"> {"{{ items }}"} </span> for batch dispatch, and
              <span className="font-mono"> {"{{ queue }}"} </span> for queue metadata.
            </p>
            {errors.action_params && (
              <p className="mt-1 text-sm text-red-600">{errors.action_params}</p>
            )}
          </div>

          <div className="rounded-lg border border-gray-200 p-4">
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Execution API token permissions
            </label>
            <select
              value={permissionMode}
              onChange={(e) =>
                setPermissionMode(e.target.value as PermissionOverrideMode)
              }
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="inherit">Inherit dispatch action default</option>
              <option value="none">No execution API token</option>
              <option value="custom">Custom permission set refs</option>
            </select>
            {permissionMode === "custom" && (
              <input
                value={permissionSetRefsInput}
                onChange={(e) => setPermissionSetRefsInput(e.target.value)}
                className="mt-3 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                placeholder="standard, core.queue_worker"
              />
            )}
            <p className="mt-2 text-xs text-gray-500">
              Applies to executions dispatched from this queue. Custom refs are
              comma-separated; use <span className="font-mono">standard</span> for
              action/pack-scoped keys and artifacts.
            </p>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Queue config (JSON)
            </label>
            <textarea
              value={config}
              onChange={(e) => setConfig(e.target.value)}
              rows={12}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
            />
            <p className="mt-1 text-xs text-gray-500">
              Queue config now covers tunables and ack behavior. Parameter mapping belongs in action parameters above.
            </p>
            {errors.config && <p className="mt-1 text-sm text-red-600">{errors.config}</p>}
          </div>
        </div>
      </div>

      <div className="flex items-center justify-end gap-3">
        <button
          type="button"
          onClick={() => navigate(isEditing && initialData ? `/queues/${encodeURIComponent(initialData.ref)}` : "/queues")}
          className="px-4 py-2 rounded-lg bg-gray-100 text-gray-700 hover:bg-gray-200 transition-colors"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={isSubmitting}
          className="px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {isSubmitting ? "Saving..." : isEditing ? "Save Changes" : "Create Queue"}
        </button>
      </div>
    </form>
  );
}
