import { useState, useEffect } from "react";
import { useKey, useUpdateKey } from "@/hooks/useKeys";
import { formatJsonValue } from "@/lib/format-utils";
import { X, Eye, EyeOff } from "lucide-react";
import KeyOwnerDisplay from "./KeyOwnerDisplay";
import ValueSchemaEditor from "@/components/common/ValueSchemaEditor";
import ContractValueForm from "@/components/common/ContractValueForm";
import { SchemaPreviewContext } from "@/api/models/SchemaPreviewContext";
import { toJsonValue } from "@/lib/json-value";

interface KeyEditModalProps {
  keyRef: string;
  onClose: () => void;
}

function parseKeyValue(value: string, originalValue: unknown): unknown {
  return typeof originalValue === "string" ? value : JSON.parse(value);
}

export default function KeyEditModal({ keyRef, onClose }: KeyEditModalProps) {
  const { data: keyData, isLoading } = useKey(keyRef);
  const key = keyData?.data;

  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [encrypted, setEncrypted] = useState(true);
  const [showValue, setShowValue] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [schemaChange, setSchemaChange] = useState<
    { value: unknown } | undefined
  >();
  const [schemaError, setSchemaError] = useState<string | null>(null);
  const [schemaDigest, setSchemaDigest] = useState<string | undefined>();
  const [rebindSchema, setRebindSchema] = useState(false);
  const [structuredEdit, setStructuredEdit] = useState<
    { value: unknown } | undefined
  >();

  const updateKeyMutation = useUpdateKey();

  /* eslint-disable react-hooks/set-state-in-effect -- sync local form state from fetched key data */
  useEffect(() => {
    if (key) {
      setName(key.name);
      setValue(formatJsonValue(key.value, 2));
      setEncrypted(key.encrypted);
    }
  }, [key]);
  /* eslint-enable react-hooks/set-state-in-effect */

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    if (schemaError) {
      setError(schemaError);
      return;
    }

    try {
      const valueChanged = value !== formatJsonValue(key?.value, 2);
      const encryptionChanged = encrypted !== key?.encrypted;

      await updateKeyMutation.mutateAsync({
        ref: keyRef,
        data: {
          name: name !== key?.name ? name : undefined,
          value: structuredEdit
            ? toJsonValue(structuredEdit.value)
            : valueChanged
              ? toJsonValue(parseKeyValue(value, key?.value))
              : undefined,
          encrypted: encryptionChanged ? encrypted : undefined,
          ...(schemaChange
            ? { value_schema: toJsonValue(schemaChange.value) }
            : {}),
          ...(rebindSchema ? { rebind_schema: true } : {}),
          ...(schemaDigest ? { schema_digest: schemaDigest } : {}),
        },
      });
      onClose();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to update key");
    }
  };

  if (isLoading) {
    return (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
        <div className="bg-white rounded-lg shadow-xl p-6">
          <div className="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
          <p className="mt-4 text-gray-600">Loading key...</p>
        </div>
      </div>
    );
  }

  if (!key) {
    return (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
        <div className="bg-white rounded-lg shadow-xl p-6">
          <p className="text-red-600">Key not found</p>
          <button
            onClick={onClose}
            className="mt-4 px-4 py-2 bg-gray-100 text-gray-700 rounded-lg hover:bg-gray-200"
          >
            Close
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
      <div className="bg-white rounded-lg shadow-xl max-w-2xl w-full max-h-[90vh] overflow-y-auto">
        <div className="flex items-center justify-between p-6 border-b border-gray-200">
          <h2 className="text-2xl font-bold text-gray-900">
            Edit Key: {keyRef}
          </h2>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 transition-colors"
          >
            <X className="w-6 h-6" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="p-6 space-y-4">
          <ValueSchemaEditor
            key={key.ref}
            value={key.value_schema}
            onChange={(value) => setSchemaChange({ value })}
            onError={setSchemaError}
            onDigest={setSchemaDigest}
          />
          {key.value_schema != null && (
            <label className="flex gap-2 text-sm">
              <input
                type="checkbox"
                checked={rebindSchema}
                onChange={(event) => setRebindSchema(event.target.checked)}
              />
              Rebind to current installed type revisions and revalidate the
              complete value
            </label>
          )}
          {key.schema_digest && (
            <p className="text-xs text-gray-600 break-all">
              Assigned contract: {key.schema_digest}
            </p>
          )}
          {error && (
            <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded-lg">
              {error}
            </div>
          )}

          <div className="bg-gray-50 border border-gray-200 rounded-lg p-4 space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-gray-500">
                Reference:
              </span>
              <span className="text-sm font-mono text-gray-900">{key.ref}</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-gray-500">
                Local reference:
              </span>
              <span className="text-sm font-mono text-gray-900">
                {key.local_ref}
              </span>
            </div>
            <div className="flex items-center justify-between gap-4">
              <span className="text-sm font-medium text-gray-500">Owner:</span>
              <KeyOwnerDisplay
                ownerType={key.owner_type}
                ownerRef={key.owner}
              />
            </div>
          </div>

          <div>
            <label
              htmlFor="name"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Name <span className="text-red-500">*</span>
            </label>
            <input
              id="name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>

          <div>
            <label
              htmlFor="value"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Value <span className="text-red-500">*</span>
            </label>
            {(schemaChange
              ? schemaChange.value != null
              : key.value_schema != null) && (
              <button
                type="button"
                onClick={() =>
                  setStructuredEdit({ value: key.encrypted ? "" : key.value })
                }
                className="text-sm text-blue-600"
              >
                Replace complete value with schema controls
              </button>
            )}
            {structuredEdit && (
              <ContractValueForm
                schema={schemaChange ? schemaChange.value : key.value_schema}
                contract={
                  !schemaChange && !rebindSchema
                    ? key.schema_binding
                    : undefined
                }
                context={SchemaPreviewContext.SINGLE_VALUE}
                value={structuredEdit.value}
                onChange={(value) => setStructuredEdit({ value })}
                maskValue={encrypted}
                disabled={updateKeyMutation.isPending}
              />
            )}
            {structuredEdit && (
              <button
                type="button"
                onClick={() => setStructuredEdit(undefined)}
              >
                Keep existing value
              </button>
            )}
            <div className="relative" hidden={structuredEdit !== undefined}>
              <textarea
                id="value"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                required={structuredEdit === undefined}
                rows={6}
                className={`w-full px-3 py-2 pr-10 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm ${
                  !showValue ? "text-security-disc" : ""
                }`}
              />
              <button
                type="button"
                onClick={() => setShowValue(!showValue)}
                className="absolute right-2 top-2 text-gray-400 hover:text-gray-600"
                title={showValue ? "Hide value" : "Show value"}
              >
                {showValue ? (
                  <EyeOff className="w-5 h-5" />
                ) : (
                  <Eye className="w-5 h-5" />
                )}
              </button>
            </div>
            <p className="mt-1 text-xs text-gray-500">
              {key.encrypted
                ? "Current value is encrypted in database"
                : "Current value is stored as plain text"}
            </p>
          </div>

          <div className="flex items-center">
            <input
              id="encrypted"
              type="checkbox"
              checked={encrypted}
              onChange={(e) => setEncrypted(e.target.checked)}
              className="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
            />
            <label
              htmlFor="encrypted"
              className="ml-2 block text-sm text-gray-900"
            >
              Encrypt value (recommended for secrets)
            </label>
          </div>

          {encrypted !== key.encrypted && (
            <div className="bg-yellow-50 border border-yellow-200 text-yellow-800 px-4 py-3 rounded-lg text-sm">
              {encrypted
                ? "⚠️ Changing from unencrypted to encrypted will re-encrypt the value"
                : "⚠️ Warning: Changing from encrypted to unencrypted will store the value as plain text"}
            </div>
          )}

          <div className="flex items-center justify-end gap-3 pt-4 border-t border-gray-200">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-gray-700 bg-gray-100 rounded-lg hover:bg-gray-200 transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={updateKeyMutation.isPending}
              className="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {updateKeyMutation.isPending ? "Saving..." : "Save Changes"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
