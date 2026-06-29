import { useState } from "react";
import { X } from "lucide-react";
import ParamSchemaForm, {
  extractProperties,
  type ParamSchema,
  validateParamSchema,
} from "@/components/common/ParamSchemaForm";
import { type JsonValue, type WorkQueueItemResponse } from "@/api/queues";
import { useEnqueueQueueItem, useUpdateQueueItem } from "@/hooks/useQueues";
import { parseJsonObject, parseJsonValue, prettyJson } from "./queueUtils";

interface QueueItemModalProps {
  queueRef: string;
  itemSchema?: JsonValue;
  defaultPriority?: number;
  item?: WorkQueueItemResponse | null;
  onClose: () => void;
}

function isJsonObject(
  value: JsonValue | undefined | null,
): value is Record<string, JsonValue> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function splitPayloadBySchema(
  payload: JsonValue | undefined,
  schema: ParamSchema,
): {
  formValues: Record<string, JsonValue>;
  extraValues: Record<string, JsonValue>;
} {
  if (!isJsonObject(payload)) {
    return { formValues: {}, extraValues: {} };
  }

  const schemaKeys = new Set(Object.keys(extractProperties(schema)));
  const formValues: Record<string, JsonValue> = {};
  const extraValues: Record<string, JsonValue> = {};

  for (const [key, value] of Object.entries(payload)) {
    if (schemaKeys.has(key)) {
      formValues[key] = value;
    } else {
      extraValues[key] = value;
    }
  }

  return { formValues, extraValues };
}

function getErrorMessage(error: unknown, fallback: string): string {
  const maybeAxios = error as { response?: { data?: { message?: string } } };
  return (
    maybeAxios.response?.data?.message ||
    (error instanceof Error ? error.message : fallback)
  );
}

export default function QueueItemModal({
  queueRef,
  itemSchema,
  defaultPriority = 0,
  item,
  onClose,
}: QueueItemModalProps) {
  const enqueueItem = useEnqueueQueueItem();
  const updateItem = useUpdateQueueItem();
  const isEditing = !!item;
  const payloadSchema: ParamSchema = (itemSchema as ParamSchema) || {};
  const hasPayloadSchema =
    Object.keys(extractProperties(payloadSchema)).length > 0;
  const initialPayload = isEditing ? item?.payload : {};
  const supportsStructuredPayload =
    hasPayloadSchema &&
    (isJsonObject(initialPayload) || initialPayload === undefined);
  const initialPayloadParts = splitPayloadBySchema(
    initialPayload,
    payloadSchema,
  );

  const [itemKey, setItemKey] = useState(item?.item_key ?? "");
  const [priority, setPriority] = useState(
    () => item?.priority ?? defaultPriority,
  );
  const [payloadValues, setPayloadValues] = useState<Record<string, JsonValue>>(
    initialPayloadParts.formValues,
  );
  const [payloadErrors, setPayloadErrors] = useState<Record<string, string>>(
    {},
  );
  const [extraPayload, setExtraPayload] = useState(() =>
    prettyJson(initialPayloadParts.extraValues),
  );
  const [payload, setPayload] = useState(() => prettyJson(item?.payload, {}));
  const [metadata, setMetadata] = useState(() =>
    prettyJson(item?.metadata, {}),
  );
  const [error, setError] = useState<string | null>(null);

  const isSubmitting = enqueueItem.isPending || updateItem.isPending;

  const handlePayloadValuesChange = (nextValues: Record<string, JsonValue>) => {
    setPayloadValues(nextValues);
    if (Object.keys(payloadErrors).length > 0) {
      setPayloadErrors({});
    }
  };

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);

    let parsedPayload;
    let parsedMetadata;
    try {
      if (supportsStructuredPayload) {
        const validationErrors = validateParamSchema(
          payloadSchema,
          payloadValues,
        );
        setPayloadErrors(validationErrors);
        if (Object.keys(validationErrors).length > 0) {
          setError("Please correct the payload form errors.");
          return;
        }

        parsedPayload = {
          ...parseJsonObject("Additional payload fields", extraPayload),
          ...payloadValues,
        };
      } else {
        parsedPayload = parseJsonValue("Payload", payload);
      }
      parsedMetadata = parseJsonObject("Metadata", metadata);
    } catch (parseError) {
      setError(
        parseError instanceof Error
          ? parseError.message
          : "Invalid queue item JSON",
      );
      return;
    }

    try {
      setPayloadErrors({});
      if (item) {
        await updateItem.mutateAsync({
          ref: queueRef,
          itemId: item.id,
          data: {
            item_key: itemKey.trim()
              ? { op: "set", value: itemKey.trim() }
              : { op: "clear" },
            priority,
            payload: parsedPayload,
            metadata: parsedMetadata,
          },
        });
      } else {
        await enqueueItem.mutateAsync({
          ref: queueRef,
          data: {
            item_key: itemKey.trim() || null,
            priority,
            payload: parsedPayload,
            metadata: parsedMetadata,
          },
        });
      }
      onClose();
    } catch (submitError) {
      setError(
        getErrorMessage(
          submitError,
          isEditing ? "Failed to update queue item" : "Failed to enqueue item",
        ),
      );
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div className="w-full max-w-3xl max-h-[90vh] overflow-y-auto rounded-lg bg-white shadow-xl">
        <div className="flex items-center justify-between border-b border-gray-200 p-6">
          <div>
            <h2 className="text-2xl font-bold text-gray-900">
              {isEditing ? `Edit Queue Item #${item?.id}` : "Add Queue Item"}
            </h2>
            <p className="mt-1 text-sm text-gray-500 font-mono">{queueRef}</p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600"
          >
            <X className="h-6 w-6" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4 p-6">
          {error && (
            <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
              {error}
            </div>
          )}

          <div className="grid gap-4 md:grid-cols-2">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Item key
              </label>
              <input
                value={itemKey}
                onChange={(e) => setItemKey(e.target.value)}
                className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                placeholder="order-123"
              />
              <p className="mt-1 text-xs text-gray-500">
                Optional deduplication key for pending updates.
              </p>
            </div>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Priority
              </label>
              <input
                type="number"
                value={priority}
                onChange={(e) => setPriority(Number(e.target.value))}
                className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Payload
            </label>
            {supportsStructuredPayload ? (
              <div className="space-y-4 rounded-lg border border-gray-200 bg-gray-50 p-4">
                <p className="text-xs text-gray-500">
                  Fill in the queue item fields defined by this queue&apos;s
                  item schema.
                </p>
                <ParamSchemaForm
                  schema={payloadSchema}
                  values={payloadValues}
                  onChange={handlePayloadValuesChange}
                  errors={payloadErrors}
                />
                <div>
                  <label className="mb-1 block text-sm font-medium text-gray-700">
                    Additional payload fields (JSON object)
                  </label>
                  <textarea
                    value={extraPayload}
                    onChange={(e) => setExtraPayload(e.target.value)}
                    rows={4}
                    className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                  />
                  <p className="mt-1 text-xs text-gray-500">
                    Optional extra payload keys not covered by the queue item
                    schema.
                  </p>
                </div>
              </div>
            ) : (
              <>
                <textarea
                  value={payload}
                  onChange={(e) => setPayload(e.target.value)}
                  rows={10}
                  className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
                />
                {hasPayloadSchema && (
                  <p className="mt-1 text-xs text-amber-600">
                    This item&apos;s payload does not match the queue item
                    schema shape, so the raw JSON editor is shown instead.
                  </p>
                )}
              </>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Metadata (JSON object)
            </label>
            <textarea
              value={metadata}
              onChange={(e) => setMetadata(e.target.value)}
              rows={6}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm"
            />
          </div>

          <div className="flex items-center justify-end gap-3 border-t border-gray-200 pt-4">
            <button
              type="button"
              onClick={onClose}
              className="rounded-lg bg-gray-100 px-4 py-2 text-gray-700 hover:bg-gray-200 transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={isSubmitting}
              className="rounded-lg bg-blue-600 px-4 py-2 text-white hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {isSubmitting
                ? "Saving..."
                : isEditing
                  ? "Save Item"
                  : "Enqueue Item"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
