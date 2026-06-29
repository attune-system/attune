import type { JsonValue, WorkQueueResponse } from "@/api/queues";
import {
  extractProperties,
  type ParamSchema,
  type ParamSchemaProperty,
} from "@/components/common/ParamSchemaForm";
import {
  formatQueueInterExecutionDelay,
  formatQueueRetryLimit,
  formatQueueTunable,
  getQueueBatchCoalescingSummary,
  getUpdateStrategyLabel,
  parseQueueConfig,
  prettyJson,
  formatJsonPreview,
  resolveQueueTunableNumber,
} from "./queueUtils";

interface QueueConfigSummaryProps {
  queue: WorkQueueResponse;
  dispatchActionParamSchema?: ParamSchema | Record<string, unknown> | null;
  showRawJson?: boolean;
}

function ConfigCard({
  label,
  value,
  muted,
  className = "",
}: {
  label: string;
  value: React.ReactNode;
  muted?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={`rounded-lg border border-gray-200 bg-gray-50 p-4 ${className}`.trim()}
    >
      <h3 className="text-xs font-semibold uppercase tracking-wide text-gray-500">
        {label}
      </h3>
      <div className="mt-2 text-sm font-medium text-gray-900">{value}</div>
      {muted && <div className="mt-1 text-xs text-gray-500">{muted}</div>}
    </div>
  );
}

function RawJsonBlock({ value }: { value: unknown }) {
  return (
    <pre className="overflow-x-auto rounded-lg bg-white p-3 text-xs text-gray-800">
      {prettyJson(value)}
    </pre>
  );
}

function QueueSchemaPreview({
  schema,
  showRawJson = false,
}: {
  schema: WorkQueueResponse["item_schema"];
  showRawJson?: boolean;
}) {
  if (showRawJson) {
    return <RawJsonBlock value={schema} />;
  }

  const schemaObject =
    schema && typeof schema === "object" && !Array.isArray(schema)
      ? (schema as Record<string, unknown>)
      : undefined;
  const fields = Object.entries(extractProperties(schemaObject)).sort(
    ([left], [right]) => left.localeCompare(right),
  );

  if (fields.length === 0) {
    return (
      <div className="text-sm font-medium text-gray-600">
        No queue item schema defined.
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {fields.map(([name, field]) => (
        <div
          key={name}
          className="rounded-md border border-gray-200 bg-white px-3 py-2"
        >
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-mono text-sm font-semibold text-gray-900">
              {name}
            </span>
            <span className="rounded bg-blue-50 px-2 py-0.5 text-[11px] font-medium text-blue-700">
              {field.type || "string"}
            </span>
            {field.required && (
              <span className="rounded bg-red-50 px-2 py-0.5 text-[11px] font-medium text-red-700">
                Required
              </span>
            )}
            {field.secret && (
              <span className="rounded bg-yellow-50 px-2 py-0.5 text-[11px] font-medium text-yellow-700">
                Secret
              </span>
            )}
          </div>

          {field.description && (
            <p className="mt-1 text-xs text-gray-600">{field.description}</p>
          )}

          {(field.default !== undefined || field.enum?.length) && (
            <div className="mt-2 flex flex-wrap gap-3 text-[11px] text-gray-500">
              {field.default !== undefined && (
                <span className="font-mono">
                  Default: {JSON.stringify(field.default)}
                </span>
              )}
              {field.enum?.length ? (
                <span className="font-mono">Enum: {field.enum.join(", ")}</span>
              ) : null}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

function ActionParamMetadata({
  definition,
}: {
  definition?: ParamSchemaProperty;
}) {
  return (
    <>
      <span className="rounded bg-blue-50 px-2 py-0.5 text-[11px] font-medium text-blue-700">
        {definition?.type || "unknown"}
      </span>
      {definition?.required && (
        <span className="rounded bg-red-50 px-2 py-0.5 text-[11px] font-medium text-red-700">
          Required
        </span>
      )}
      {definition?.secret && (
        <span className="rounded bg-yellow-50 px-2 py-0.5 text-[11px] font-medium text-yellow-700">
          Secret
        </span>
      )}
    </>
  );
}

function ActionParamsPreview({
  actionParams,
  paramSchema,
  showRawJson = false,
}: {
  actionParams: WorkQueueResponse["action_params"];
  paramSchema?: ParamSchema | Record<string, unknown> | null;
  showRawJson?: boolean;
}) {
  if (showRawJson) {
    return <RawJsonBlock value={actionParams} />;
  }

  if (
    !actionParams ||
    typeof actionParams !== "object" ||
    Array.isArray(actionParams)
  ) {
    return (
      <div className="text-sm font-medium text-gray-600">
        Uses the default queue dispatch input contract.
      </div>
    );
  }

  const entries = Object.entries(
    actionParams as Record<string, JsonValue>,
  ).sort(([left], [right]) => left.localeCompare(right));

  if (entries.length === 0) {
    return (
      <div className="text-sm font-medium text-gray-600">
        Uses the default queue dispatch input contract.
      </div>
    );
  }

  const properties = extractProperties(paramSchema);

  return (
    <div className="space-y-2">
      {entries.map(([name, value]) => {
        const definition = properties[name];

        return (
          <div
            key={name}
            className="rounded-md border border-gray-200 bg-white px-3 py-2"
          >
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-sm font-semibold text-gray-900">
                {name}
              </span>
              <ActionParamMetadata definition={definition} />
            </div>
            {definition?.description && (
              <p className="mt-1 text-xs text-gray-600">
                {definition.description}
              </p>
            )}
            <div className="mt-2 rounded bg-gray-50 px-2 py-1 font-mono text-xs font-medium text-gray-700">
              {formatJsonPreview(value, 144)}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function QueueConfigPreview({
  queue,
  showRawJson = false,
}: {
  queue: WorkQueueResponse;
  showRawJson?: boolean;
}) {
  if (showRawJson) {
    return <RawJsonBlock value={queue.config} />;
  }

  const config = parseQueueConfig(queue.config);
  const resolvedDispatchTuning = queue.resolved_dispatch_tuning;
  const concurrency = formatQueueTunable(
    config.dispatch?.concurrency,
    "Default: 1",
    resolvedDispatchTuning?.concurrency,
  );
  const effectiveConcurrency = resolveQueueTunableNumber(
    config.dispatch?.concurrency,
    resolvedDispatchTuning?.concurrency,
  );
  const batchSize =
    queue.batch_mode === "batch"
      ? formatQueueTunable(
          config.dispatch?.batch_size,
          "Default: 1",
          resolvedDispatchTuning?.batch_size,
        )
      : "Single item dispatch";
  const priority = queue.default_priority.toString();
  const ackContract = config.ack_contract?.version
    ? `Version ${config.ack_contract.version}`
    : "Not required";
  const coalescing = getQueueBatchCoalescingSummary(config, queue.batch_mode);
  const effectiveBatchSize = resolveQueueTunableNumber(
    config.dispatch?.batch_size,
    resolvedDispatchTuning?.batch_size,
  );
  const showBatchingOptions =
    queue.batch_mode === "batch" &&
    effectiveBatchSize !== null &&
    effectiveBatchSize > 1;

  const rows = [
    { label: "Concurrency", value: concurrency },
    {
      label: "Sequential delay",
      value: formatQueueInterExecutionDelay(
        config.dispatch?.inter_execution_delay_seconds,
        effectiveConcurrency,
      ),
    },
    {
      label: "Retry limit",
      value: formatQueueRetryLimit(config.dispatch?.retry_limit),
    },
    {
      label: (
        <span className="inline-flex flex-wrap items-center gap-2">
          <span>Batch size</span>
          {showBatchingOptions && (
            <span className="rounded bg-emerald-50 px-2 py-0.5 text-[11px] font-medium normal-case tracking-normal text-emerald-700">
              Batch mode enabled
            </span>
          )}
        </span>
      ),
      value: <span>{batchSize}</span>,
    },
    {
      label: "Pending update",
      value: queue.allow_pending_update ? "Allowed" : "Rejected",
    },
    {
      label: "Update strategy",
      value: getUpdateStrategyLabel(queue.update_strategy),
    },
    { label: "Priority", value: priority },
    { label: "Ack contract", value: ackContract },
  ];

  if (showBatchingOptions) {
    rows.splice(
      2,
      0,
      { label: "Batch coalescing", value: coalescing.statusLabel },
      {
        label: "Coalescing group",
        value: coalescing.enabled ? coalescing.groupByPath || "Unset" : "—",
      },
      {
        label: "Coalesce batches across priorities",
        value: coalescing.enabled
          ? coalescing.acrossPriorities
            ? "Yes"
            : "No"
          : "—",
      },
    );
  }

  return (
    <dl className="space-y-2">
      {rows.map((row, index) => (
        <div
          key={typeof row.label === "string" ? row.label : `row-${index}`}
          className="flex items-start justify-between gap-4 border-b border-gray-200 pb-2 last:border-b-0 last:pb-0"
        >
          <dt className="text-xs font-medium uppercase tracking-wide text-gray-500">
            {row.label}
          </dt>
          <dd className="text-right text-sm font-medium text-gray-900">
            {row.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

export default function QueueConfigSummary({
  queue,
  dispatchActionParamSchema,
  showRawJson = false,
}: QueueConfigSummaryProps) {
  return (
    <div className="space-y-5">
      <div className="grid gap-4 md:grid-cols-2">
        <ConfigCard
          label="Queue item schema"
          value={
            <QueueSchemaPreview
              schema={queue.item_schema}
              showRawJson={showRawJson}
            />
          }
          muted={
            showRawJson
              ? "Persisted queue item schema JSON."
              : "Uses the same flat schema format as triggers."
          }
        />
        <ConfigCard
          label="Action params"
          value={
            <ActionParamsPreview
              actionParams={queue.action_params}
              paramSchema={dispatchActionParamSchema}
              showRawJson={showRawJson}
            />
          }
          muted={
            showRawJson
              ? "Persisted queue action parameter mapping JSON."
              : "Resolved against the dispatch action parameter schema."
          }
        />
        <ConfigCard
          label="Queue config"
          value={<QueueConfigPreview queue={queue} showRawJson={showRawJson} />}
          muted={
            showRawJson
              ? "Persisted queue config JSON."
              : "Dispatch, batching, and acknowledgement settings."
          }
        />
      </div>
    </div>
  );
}
