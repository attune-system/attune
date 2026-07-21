import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useAction, useActions } from "@/hooks/useActions";
import { usePacks } from "@/hooks/usePacks";
import { useCreateQueue, useUpdateQueue } from "@/hooks/useQueues";
import ParamSchemaForm, {
  validateParamSchema,
  type ParamSchema,
} from "@/components/common/ParamSchemaForm";
import SchemaBuilder from "@/components/common/SchemaBuilder";
import SearchableSelect from "@/components/common/SearchableSelect";
import { combineRefs, extractLocalRef, labelToRef } from "@/lib/format-utils";
import {
  WorkQueueBatchMode,
  WorkQueueUpdateStrategy,
  ReferenceVisibility,
  type JsonValue,
  type WorkQueueResponse,
} from "@/api/queues";
import { parseQueueConfig } from "./queueUtils";

interface QueueFormProps {
  initialData?: WorkQueueResponse;
  isEditing?: boolean;
}

type PermissionOverrideMode = "inherit" | "none" | "custom";
type TunableSource = "literal" | "pack_config" | "keystore";
type JsonObject = Record<string, JsonValue>;
type FlatSchema = Record<string, Record<string, unknown>>;

function getErrorMessage(error: unknown, fallback: string): string {
  const maybeAxios = error as { response?: { data?: { message?: string } } };
  return (
    maybeAxios.response?.data?.message ||
    (error instanceof Error ? error.message : fallback)
  );
}

function asJsonObject(value: JsonValue | null | undefined): JsonObject {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as JsonObject;
  }
  return {};
}

function asFlatSchema(value: JsonValue | null | undefined): FlatSchema {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as FlatSchema;
  }
  return {};
}

function valueToInput(value: JsonValue | undefined): string {
  if (value === undefined || value === null) {
    return "";
  }
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

function parseOptionalPositiveInteger(
  label: string,
  raw: string,
  required = false,
): number | undefined {
  const trimmed = raw.trim();
  if (!trimmed) {
    if (required) {
      throw new Error(`${label} is required`);
    }
    return undefined;
  }

  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${label} must be a positive integer`);
  }
  return parsed;
}

function parseOptionalNonNegativeInteger(
  label: string,
  raw: string,
): number | undefined {
  const trimmed = raw.trim();
  if (!trimmed) {
    return undefined;
  }

  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative integer`);
  }
  return parsed;
}

export default function QueueForm({
  initialData,
  isEditing = false,
}: QueueFormProps) {
  const navigate = useNavigate();
  const createQueue = useCreateQueue();
  const updateQueue = useUpdateQueue();

  const { data: packsData } = usePacks({ pageSize: 1000 });
  const packs = packsData?.items ?? [];
  const [packId, setPackId] = useState(() => initialData?.pack ?? 0);
  const selectedPack = packs.find((pack) => pack.id === packId);
  const [localRef, setLocalRef] = useState(() =>
    initialData?.ref
      ? extractLocalRef(initialData.ref, initialData.pack_ref ?? undefined)
      : "",
  );
  const [label, setLabel] = useState(() => initialData?.label ?? "");
  const [description, setDescription] = useState(
    () => initialData?.description ?? "",
  );
  const [dispatchActionRef, setDispatchActionRef] = useState(
    () => initialData?.dispatch_action_ref ?? "",
  );
  const [traceTagTemplate, setTraceTagTemplate] = useState(
    () => initialData?.trace_tag_template ?? "",
  );
  const { data: actionsData } = useActions({
    page: 1,
    pageSize: 200,
    referencingPackRef: selectedPack?.ref,
  });
  const { data: selectedActionData, isFetching: isFetchingSelectedAction } =
    useAction(dispatchActionRef);
  const [enabled, setEnabled] = useState(() => initialData?.enabled ?? true);
  const [acceptingNewItems, setAcceptingNewItems] = useState(
    () => initialData?.accepting_new_items ?? true,
  );
  const [referenceVisibility, setReferenceVisibility] =
    useState<ReferenceVisibility>(
      () => initialData?.reference_visibility ?? ReferenceVisibility.PUBLIC,
    );
  const [referenceAllowedPackRefs, setReferenceAllowedPackRefs] = useState(() =>
    (initialData?.reference_allowed_pack_refs ?? []).join("\n"),
  );
  const [defaultPriority, setDefaultPriority] = useState(
    () => initialData?.default_priority ?? 10,
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
  const [itemSchema, setItemSchema] = useState<FlatSchema>(() =>
    asFlatSchema(initialData?.item_schema),
  );
  const [actionParams, setActionParams] = useState<JsonObject>(() =>
    asJsonObject(initialData?.action_params),
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
  const initialQueueConfig = parseQueueConfig(initialData?.config);
  const initialConcurrency = initialQueueConfig.dispatch?.concurrency;
  const initialBatchSize = initialQueueConfig.dispatch?.batch_size;
  const [advancedConfigOpen, setAdvancedConfigOpen] = useState(false);
  const [concurrencySource, setConcurrencySource] = useState<TunableSource>(
    () => initialConcurrency?.source ?? "literal",
  );
  const [concurrencyValue, setConcurrencyValue] = useState(() =>
    valueToInput(initialConcurrency?.value ?? 1),
  );
  const [concurrencyPath, setConcurrencyPath] = useState(
    () => initialConcurrency?.path ?? "",
  );
  const [concurrencyKeyRef, setConcurrencyKeyRef] = useState(
    () => initialConcurrency?.key_ref ?? "",
  );
  const [concurrencyFallback, setConcurrencyFallback] = useState(() =>
    valueToInput(initialConcurrency?.fallback),
  );
  const [batchSizeSource, setBatchSizeSource] = useState<TunableSource>(
    () => initialBatchSize?.source ?? "literal",
  );
  const [batchSizeValue, setBatchSizeValue] = useState(() =>
    valueToInput(initialBatchSize?.value ?? 1),
  );
  const [batchSizePath, setBatchSizePath] = useState(
    () => initialBatchSize?.path ?? "",
  );
  const [batchSizeKeyRef, setBatchSizeKeyRef] = useState(
    () => initialBatchSize?.key_ref ?? "",
  );
  const [batchSizeFallback, setBatchSizeFallback] = useState(() =>
    valueToInput(initialBatchSize?.fallback),
  );
  const [ackContractVersion, setAckContractVersion] = useState(() =>
    initialQueueConfig.ack_contract?.version !== undefined
      ? String(initialQueueConfig.ack_contract.version)
      : "1",
  );
  const [coalescingEnabled, setCoalescingEnabled] = useState(
    () => initialQueueConfig.dispatch?.coalescing?.enabled ?? false,
  );
  const [interExecutionDelaySeconds, setInterExecutionDelaySeconds] = useState(
    () =>
      initialQueueConfig.dispatch?.inter_execution_delay_seconds !== undefined
        ? String(initialQueueConfig.dispatch.inter_execution_delay_seconds)
        : "",
  );
  const [retryLimit, setRetryLimit] = useState(() =>
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
  const [actionParamFieldErrors, setActionParamFieldErrors] = useState<
    Record<string, string>
  >({});
  const isImmutableStrategy =
    updateStrategy === WorkQueueUpdateStrategy.IMMUTABLE;
  const effectiveAllowPendingUpdate = isImmutableStrategy
    ? false
    : allowPendingUpdate;

  const buildTunableConfig = (
    label: string,
    source: TunableSource,
    literalValue: string,
    path: string,
    keyRef: string,
    fallback: string,
  ): JsonObject => {
    const tunable: JsonObject = { source };

    if (source === "literal") {
      const parsedValue = parseOptionalPositiveInteger(
        label,
        literalValue,
        true,
      );
      if (parsedValue === undefined) {
        throw new Error(`${label} is required`);
      }
      tunable.value = parsedValue;
      return tunable;
    }

    if (source === "pack_config") {
      if (!path.trim()) {
        throw new Error(`${label} pack config path is required`);
      }
      tunable.path = path.trim();
    }

    if (source === "keystore") {
      if (!keyRef.trim()) {
        throw new Error(`${label} key ref is required`);
      }
      tunable.key_ref = keyRef.trim();
      if (path.trim()) {
        tunable.path = path.trim();
      }
    }

    const parsedFallback = parseOptionalPositiveInteger(
      `${label} fallback`,
      fallback,
    );
    if (parsedFallback !== undefined) {
      tunable.fallback = parsedFallback;
    }

    return tunable;
  };

  const buildQueueConfig = (): JsonObject => {
    const dispatch: JsonObject = {
      concurrency: buildTunableConfig(
        "Concurrency",
        concurrencySource,
        concurrencyValue,
        concurrencyPath,
        concurrencyKeyRef,
        concurrencyFallback,
      ),
    };

    if (batchMode === WorkQueueBatchMode.BATCH) {
      dispatch.batch_size = buildTunableConfig(
        "Batch size",
        batchSizeSource,
        batchSizeValue,
        batchSizePath,
        batchSizeKeyRef,
        batchSizeFallback,
      );
    }

    const parsedRetryLimit = parseOptionalNonNegativeInteger(
      "Retry limit",
      retryLimit,
    );
    if (parsedRetryLimit !== undefined) {
      dispatch.retry_limit = parsedRetryLimit;
    }

    const parsedDelay = parseOptionalNonNegativeInteger(
      "Sequential inter-execution delay",
      interExecutionDelaySeconds,
    );
    if (parsedDelay !== undefined) {
      dispatch.inter_execution_delay_seconds = parsedDelay;
    }

    if (
      batchMode === WorkQueueBatchMode.BATCH &&
      (coalescingEnabled ||
        coalescingGroupByPath.trim() ||
        coalescingAcrossPriorities)
    ) {
      if (coalescingEnabled && !coalescingGroupByPath.trim()) {
        throw new Error(
          "Group by payload path is required when coalescing is enabled",
        );
      }
      dispatch.coalescing = {
        enabled: coalescingEnabled,
        across_priorities: coalescingAcrossPriorities,
        ...(coalescingGroupByPath.trim()
          ? { group_by_path: coalescingGroupByPath.trim() }
          : {}),
      };
    }

    const parsedAckContractVersion = parseOptionalPositiveInteger(
      "Ack contract version",
      ackContractVersion,
      true,
    );
    if (parsedAckContractVersion === undefined) {
      throw new Error("Ack contract version is required");
    }

    return {
      dispatch,
      ack_contract: { version: parsedAckContractVersion },
    };
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
  const selectedDispatchAction = actionOptions.find(
    (action) => action.ref === dispatchActionRef,
  );
  const dispatchActionParamSchema: ParamSchema =
    (selectedActionData?.data?.param_schema as ParamSchema | null) || {};

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();

    const nextErrors: Record<string, string> = {};
    if (!localRef.trim() && !isEditing) {
      nextErrors.ref = "Queue ref is required";
    }
    if (!selectedPack && !isEditing) {
      nextErrors.pack = "Pack is required";
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

    const nextActionParamFieldErrors = validateParamSchema(
      dispatchActionParamSchema,
      actionParams,
      true,
    );
    if (Object.keys(nextActionParamFieldErrors).length > 0) {
      nextErrors.action_params = "Fix the highlighted action parameters";
    }

    let parsedConfig: JsonObject | undefined;
    try {
      parsedConfig = buildQueueConfig();
    } catch (error) {
      nextErrors.config =
        error instanceof Error ? error.message : "Queue config is invalid";
      setAdvancedConfigOpen(true);
    }

    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      setActionParamFieldErrors(nextActionParamFieldErrors);
      return;
    }

    setActionParamFieldErrors({});

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
      const fullRef = selectedPack
        ? combineRefs(selectedPack.ref, localRef.trim())
        : initialData?.ref || localRef.trim();

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
            trace_tag_template: traceTagTemplate.trim() || null,
            default_priority: defaultPriority,
            allow_pending_update: effectiveAllowPendingUpdate,
            update_strategy: updateStrategy,
            batch_mode: batchMode,
            item_schema: itemSchema as JsonValue,
            action_params: actionParams,
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
        ref: fullRef,
        pack_ref: selectedPack?.ref ?? null,
        label: label.trim(),
        description: description.trim() || null,
        enabled,
        accepting_new_items: acceptingNewItems,
        dispatch_action_ref: dispatchActionRef,
        trace_tag_template: traceTagTemplate.trim() || null,
        default_priority: defaultPriority,
        allow_pending_update: effectiveAllowPendingUpdate,
        update_strategy: updateStrategy,
        batch_mode: batchMode,
        item_schema: itemSchema as JsonValue,
        action_params: actionParams,
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

  const renderTunableControls = ({
    label,
    description,
    source,
    setSource,
    value,
    setValue,
    path,
    setPath,
    keyRef,
    setKeyRef,
    fallback,
    setFallback,
  }: {
    label: string;
    description: string;
    source: TunableSource;
    setSource: (value: TunableSource) => void;
    value: string;
    setValue: (value: string) => void;
    path: string;
    setPath: (value: string) => void;
    keyRef: string;
    setKeyRef: (value: string) => void;
    fallback: string;
    setFallback: (value: string) => void;
  }) => (
    <div className="rounded-lg border border-gray-200 p-4 space-y-3">
      <div>
        <h4 className="text-sm font-medium text-gray-900">{label}</h4>
        <p className="mt-1 text-xs text-gray-500">{description}</p>
      </div>

      <div>
        <label className="block text-sm font-medium text-gray-700 mb-1">
          Source
        </label>
        <select
          value={source}
          onChange={(e) => setSource(e.target.value as TunableSource)}
          className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
        >
          <option value="literal">Literal value</option>
          <option value="pack_config">Pack config path</option>
          <option value="keystore">Keystore key</option>
        </select>
      </div>

      {source === "literal" ? (
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Value
          </label>
          <input
            type="number"
            min={1}
            step={1}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
          />
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2">
          {source === "keystore" && (
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Key ref
              </label>
              <input
                value={keyRef}
                onChange={(e) => setKeyRef(e.target.value)}
                className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                placeholder="queue_limits"
              />
            </div>
          )}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              {source === "pack_config" ? "Pack config path" : "Value path"}
            </label>
            <input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder={
                source === "pack_config" ? "queues.max_workers" : "batch_size"
              }
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Fallback
            </label>
            <input
              type="number"
              min={1}
              step={1}
              value={fallback}
              onChange={(e) => setFallback(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="1"
            />
          </div>
        </div>
      )}
    </div>
  );

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
              Pack
            </label>
            <SearchableSelect
              value={packId}
              onChange={(value) => {
                setPackId(Number(value));
                setDispatchActionRef("");
                setActionParams({});
                setActionParamFieldErrors({});
              }}
              options={packs.map((pack) => ({
                value: pack.id,
                label: `${pack.label} (${pack.ref})`,
              }))}
              placeholder="Select a pack..."
              disabled={isEditing}
              error={!!errors.pack}
            />
            {errors.pack && (
              <p className="mt-1 text-sm text-red-600">{errors.pack}</p>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Label
            </label>
            <input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              onBlur={() => {
                if (!isEditing && !localRef.trim() && label.trim()) {
                  setLocalRef(labelToRef(label));
                }
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Manual Review Queue"
            />
            {errors.label && (
              <p className="mt-1 text-sm text-red-600">{errors.label}</p>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Queue Ref
            </label>
            <div className="flex rounded-lg border border-gray-300 focus-within:ring-2 focus-within:ring-blue-500">
              <span className="inline-flex items-center rounded-l-lg border-r border-gray-300 bg-gray-50 px-3 font-mono text-sm text-gray-500">
                {selectedPack?.ref || initialData?.pack_ref || "pack"}.
              </span>
              <input
                value={localRef}
                onChange={(e) => setLocalRef(e.target.value)}
                disabled={isEditing}
                className="min-w-0 flex-1 rounded-r-lg px-3 py-2 font-mono text-sm outline-none disabled:bg-gray-100"
                placeholder="manual_review"
              />
            </div>
            <p className="mt-1 text-xs text-gray-500">
              The full queue ref will be{" "}
              <span className="font-mono">
                {selectedPack?.ref || initialData?.pack_ref || "pack"}.
                {localRef || "manual_review"}
              </span>
              .
            </p>
            {errors.ref && (
              <p className="mt-1 text-sm text-red-600">{errors.ref}</p>
            )}
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
                Disabled queues remain visible but the executor will stop
                dispatching items from them.
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
                Disable this to reject enqueue requests while keeping current
                items intact.
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

          <div>
            <SchemaBuilder
              label="Queue item schema"
              value={itemSchema}
              onChange={setItemSchema}
              placeholder={
                '{\n  "order_id": { "type": "integer", "required": true }\n}'
              }
              error={errors.item_schema}
            />
            <p className="mt-1 text-xs text-gray-500">
              Uses the same flat schema format as triggers and is enforced when
              queue items are enqueued or updated.
            </p>
          </div>
        </div>

        <div className="bg-white rounded-lg shadow p-6 space-y-4">
          <h2 className="text-lg font-semibold text-gray-900">
            Dispatch behaviour
          </h2>

          <div>
            <label
              htmlFor="dispatch-action"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Dispatch Action
            </label>
            <SearchableSelect
              id="dispatch-action"
              value={dispatchActionRef}
              onChange={(value) => {
                setDispatchActionRef(String(value));
                setActionParams({});
                setActionParamFieldErrors({});
              }}
              options={actionOptions.map((action) => ({
                value: action.ref,
                label:
                  action.label && action.label !== action.ref
                    ? `${action.ref} — ${action.label}`
                    : action.ref,
              }))}
              placeholder="Select an action..."
              error={!!errors.dispatch_action_ref}
            />
            {errors.dispatch_action_ref && (
              <p className="mt-1 text-sm text-red-600">
                {errors.dispatch_action_ref}
              </p>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Trace tag template
            </label>
            <input
              value={traceTagTemplate}
              onChange={(e) => setTraceTagTemplate(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="e.g., {{ queue.ref }}.{{ queue.dispatch_id }}"
            />
            <p className="mt-1 text-xs text-gray-500">
              Optional. Leave blank to use defaults:
              <span className="ml-1 font-mono">
                &lt;queue_ref&gt;.&lt;work_item_id&gt;
              </span>{" "}
              for single dispatch and
              <span className="ml-1 font-mono">
                &lt;queue_ref&gt;.&lt;dispatch_id&gt;
              </span>{" "}
              for batch dispatch.
            </p>
          </div>

          <div>
            <h3 className="text-sm font-medium text-gray-700 mb-3">
              Action parameters
            </h3>
            {selectedDispatchAction &&
            isFetchingSelectedAction &&
            !selectedActionData ? (
              <div className="p-4 bg-gray-50 rounded-lg text-center text-sm text-gray-600">
                Loading action parameters...
              </div>
            ) : selectedDispatchAction ? (
              <ParamSchemaForm
                key={dispatchActionRef}
                schema={dispatchActionParamSchema}
                values={actionParams}
                onChange={(values) => {
                  setActionParams(values);
                  setActionParamFieldErrors({});
                }}
                errors={actionParamFieldErrors}
                allowTemplates
                hideTemplateHint
                templateNamespace="item"
              />
            ) : (
              <div className="p-4 bg-gray-50 rounded-lg text-center text-sm text-gray-600">
                Select a dispatch action to configure its parameters.
              </div>
            )}
            <p className="mt-1 text-xs text-gray-500">
              Use queue templates like{" "}
              <span className="font-mono">{"{{ item }}"}</span> for single
              dispatch,
              <span className="font-mono"> {"{{ items }}"} </span> for batch
              dispatch, and
              <span className="font-mono"> {"{{ queue }}"} </span> for queue
              metadata.
            </p>
            {errors.action_params && (
              <p className="mt-1 text-sm text-red-600">
                {errors.action_params}
              </p>
            )}
          </div>

          <div className="rounded-lg border border-gray-200">
            <button
              type="button"
              onClick={() => setAdvancedConfigOpen((open) => !open)}
              className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left"
              aria-expanded={advancedConfigOpen}
            >
              <span>
                <span className="block text-sm font-medium text-gray-900">
                  Advanced queue config
                </span>
                <span className="mt-1 block text-xs text-gray-500">
                  Configure dispatch tuning, update behavior, execution token
                  permissions, and queue ack contract.
                </span>
              </span>
              <span className="text-lg text-gray-500" aria-hidden="true">
                {advancedConfigOpen ? "-" : "+"}
              </span>
            </button>

            {advancedConfigOpen && (
              <div className="border-t border-gray-200 p-4 space-y-4">
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
                      When enabled, enqueue requests can update an existing
                      queued or retry item with the same key.
                    </span>
                    {isImmutableStrategy && (
                      <span className="mt-2 block text-sm text-amber-700">
                        Immutable queues always reject duplicate pending keys,
                        so pending updates are turned off.
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
                      setUpdateStrategy(
                        e.target.value as WorkQueueUpdateStrategy,
                      )
                    }
                    className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                  >
                    <option value={WorkQueueUpdateStrategy.REPLACE}>
                      Replace existing payload + metadata
                    </option>
                    <option value={WorkQueueUpdateStrategy.MERGE_PATCH}>
                      Merge patch existing payload + metadata
                    </option>
                    <option value={WorkQueueUpdateStrategy.IMMUTABLE}>
                      Reject duplicate pending item keys
                    </option>
                  </select>
                </div>

                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1">
                    Batch mode
                  </label>
                  <select
                    value={batchMode}
                    onChange={(e) =>
                      setBatchMode(e.target.value as WorkQueueBatchMode)
                    }
                    className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                  >
                    <option value={WorkQueueBatchMode.SINGLE}>
                      Single item dispatch
                    </option>
                    <option value={WorkQueueBatchMode.BATCH}>
                      Batch dispatch
                    </option>
                  </select>
                </div>

                {batchMode === WorkQueueBatchMode.BATCH && (
                  <>
                    {renderTunableControls({
                      label: "Batch size",
                      description:
                        "Maximum number of queue items leased into one execution.",
                      source: batchSizeSource,
                      setSource: setBatchSizeSource,
                      value: batchSizeValue,
                      setValue: setBatchSizeValue,
                      path: batchSizePath,
                      setPath: setBatchSizePath,
                      keyRef: batchSizeKeyRef,
                      setKeyRef: setBatchSizeKeyRef,
                      fallback: batchSizeFallback,
                      setFallback: setBatchSizeFallback,
                    })}

                    <div className="rounded-lg border border-gray-200 p-4 space-y-4">
                      <div>
                        <h3 className="text-sm font-medium text-gray-900">
                          Batch coalescing
                        </h3>
                        <p className="mt-1 text-xs text-gray-500">
                          Start from the first queued batch item, then hoist
                          later items with the same grouping value.
                        </p>
                      </div>

                      <label className="flex items-start gap-3">
                        <input
                          type="checkbox"
                          checked={coalescingEnabled}
                          onChange={(e) =>
                            setCoalescingEnabled(e.target.checked)
                          }
                          className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                        />
                        <span>
                          <span className="block text-sm font-medium text-gray-900">
                            Enable coalescing
                          </span>
                          <span className="block text-sm text-gray-500">
                            Group batch items by a payload path.
                          </span>
                        </span>
                      </label>

                      <div>
                        <label className="block text-sm font-medium text-gray-700 mb-1">
                          Group by payload path
                        </label>
                        <input
                          value={coalescingGroupByPath}
                          disabled={!coalescingEnabled}
                          onChange={(e) =>
                            setCoalescingGroupByPath(e.target.value)
                          }
                          className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm disabled:bg-gray-100"
                          placeholder="attributes.sobject_type"
                        />
                        <p className="mt-1 text-xs text-gray-500">
                          Dot-separated payload path used to group batch items,
                          such as
                          <span className="font-mono">
                            {" "}
                            attributes.sobject_type
                          </span>
                          .
                        </p>
                      </div>

                      <label className="flex items-start gap-3">
                        <input
                          type="checkbox"
                          checked={coalescingAcrossPriorities}
                          disabled={!coalescingEnabled}
                          onChange={(e) =>
                            setCoalescingAcrossPriorities(e.target.checked)
                          }
                          className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500 disabled:cursor-not-allowed"
                        />
                        <span>
                          <span className="block text-sm font-medium text-gray-900">
                            Coalesce batches across priorities
                          </span>
                          <span className="block text-sm text-gray-500">
                            When disabled, only items in the anchor item&apos;s
                            priority band may be hoisted into the batch.
                          </span>
                        </span>
                      </label>
                    </div>
                  </>
                )}

                {renderTunableControls({
                  label: "Concurrency",
                  description:
                    "Maximum number of queue executions that may run at the same time.",
                  source: concurrencySource,
                  setSource: setConcurrencySource,
                  value: concurrencyValue,
                  setValue: setConcurrencyValue,
                  path: concurrencyPath,
                  setPath: setConcurrencyPath,
                  keyRef: concurrencyKeyRef,
                  setKeyRef: setConcurrencyKeyRef,
                  fallback: concurrencyFallback,
                  setFallback: setConcurrencyFallback,
                })}

                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1">
                    Retry limit
                  </label>
                  <input
                    type="number"
                    min={0}
                    step={1}
                    value={retryLimit}
                    onChange={(e) => setRetryLimit(e.target.value)}
                    className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                    placeholder="0"
                  />
                  <p className="mt-1 text-xs text-gray-500">
                    Number of times an item may return to{" "}
                    <span className="font-mono">Retry</span> before it is marked{" "}
                    <span className="font-mono">Failed</span>.
                  </p>
                  {errors.retry_limit && (
                    <p className="mt-1 text-sm text-red-600">
                      {errors.retry_limit}
                    </p>
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
                    value={interExecutionDelaySeconds}
                    onChange={(e) =>
                      setInterExecutionDelaySeconds(e.target.value)
                    }
                    className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                    placeholder="0"
                  />
                  <p className="mt-1 text-xs text-gray-500">
                    Only applies when concurrency resolves to{" "}
                    <span className="font-mono">1</span>. The cooldown starts
                    after the prior queue execution reaches a terminal state.
                  </p>
                  {errors.inter_execution_delay_seconds && (
                    <p className="mt-1 text-sm text-red-600">
                      {errors.inter_execution_delay_seconds}
                    </p>
                  )}
                </div>

                <div className="rounded-lg border border-gray-200 p-4">
                  <label className="block text-sm font-medium text-gray-700 mb-1">
                    Execution API token permissions
                  </label>
                  <select
                    value={permissionMode}
                    onChange={(e) =>
                      setPermissionMode(
                        e.target.value as PermissionOverrideMode,
                      )
                    }
                    className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                  >
                    <option value="inherit">
                      Inherit dispatch action default
                    </option>
                    <option value="none">No execution API token</option>
                    <option value="custom">Custom permission set refs</option>
                  </select>
                  {permissionMode === "custom" && (
                    <input
                      value={permissionSetRefsInput}
                      onChange={(e) =>
                        setPermissionSetRefsInput(e.target.value)
                      }
                      className="mt-3 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                      placeholder="standard, core.queue_worker"
                    />
                  )}
                  <p className="mt-2 text-xs text-gray-500">
                    Applies to executions dispatched from this queue. Custom
                    refs are comma-separated; use{" "}
                    <span className="font-mono">standard</span> for
                    action/pack-scoped keys and artifacts.
                  </p>
                </div>

                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1">
                    Ack contract version
                  </label>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={ackContractVersion}
                    onChange={(e) => setAckContractVersion(e.target.value)}
                    className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                  />
                  <p className="mt-1 text-xs text-gray-500">
                    Version of the <span className="font-mono">queue_ack</span>{" "}
                    result shape expected from the dispatch action.
                  </p>
                </div>
              </div>
            )}
            {errors.config && (
              <p className="border-t border-red-100 px-4 py-2 text-sm text-red-600">
                {errors.config}
              </p>
            )}
          </div>
        </div>
      </div>

      <div className="flex items-center justify-end gap-3">
        <button
          type="button"
          onClick={() =>
            navigate(
              isEditing && initialData
                ? `/queues/${encodeURIComponent(initialData.ref)}`
                : "/queues",
            )
          }
          className="px-4 py-2 rounded-lg bg-gray-100 text-gray-700 hover:bg-gray-200 transition-colors"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={isSubmitting}
          className="px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {isSubmitting
            ? "Saving..."
            : isEditing
              ? "Save Changes"
              : "Create Queue"}
        </button>
      </div>
    </form>
  );
}
