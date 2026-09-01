import { useState } from "react";
import { useCreateKey } from "@/hooks/useKeys";
import { OwnerType } from "@/api";
import { X } from "lucide-react";

interface KeyCreateModalProps {
  onClose: () => void;
}

export type KeyFormat = "text" | "json" | "yaml" | "number" | "int" | "bool";

export default function KeyCreateModal({ onClose }: KeyCreateModalProps) {
  const [ref, setRef] = useState("");
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [format, setFormat] = useState<KeyFormat>("text");
  const [encrypted, setEncrypted] = useState(true);
  const [ownerType, setOwnerType] = useState<OwnerType>(OwnerType.SYSTEM);
  const [owner, setOwner] = useState("");
  const [error, setError] = useState<string | null>(null);

  const createKeyMutation = useCreateKey();

  // Determine if encryption is allowed based on format
  const canEncrypt =
    format === "text" || format === "json" || format === "yaml";

  // Auto-disable encryption for non-encryptable formats
  const isEncrypted = canEncrypt && encrypted;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);

    // Validate ref format
    if (!/^[a-zA-Z0-9_.-]+$/.test(ref)) {
      setError(
        "Reference must contain only letters, numbers, underscores, hyphens, and dots",
      );
      return;
    }

    let parsedValue: unknown = value;

    // Validate and preserve the selected JSON value type.
    try {
      if (format === "json") {
        parsedValue = JSON.parse(value);
      } else if (format === "yaml") {
        // Basic YAML validation (not exhaustive)
        if (!value.trim()) {
          throw new Error("YAML value cannot be empty");
        }
      } else if (format === "number" || format === "int") {
        const num = Number(value);
        if (isNaN(num)) {
          throw new Error(`Value must be a valid ${format}`);
        }
        if (format === "int" && !Number.isInteger(num)) {
          throw new Error("Value must be an integer");
        }
        parsedValue = num;
      } else if (format === "bool") {
        const lower = value.toLowerCase();
        if (lower !== "true" && lower !== "false") {
          throw new Error('Value must be "true" or "false"');
        }
        parsedValue = lower === "true";
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Invalid value format");
      return;
    }

    try {
      await createKeyMutation.mutateAsync({
        ref,
        name,
        value: parsedValue,
        encrypted: isEncrypted,
        owner_type: ownerType,
        ...(ownerType === OwnerType.PACK && owner
          ? { owner_pack_ref: owner }
          : {}),
        ...(ownerType === OwnerType.ACTION && owner
          ? { owner_action_ref: owner }
          : {}),
        ...(ownerType === OwnerType.SENSOR && owner
          ? { owner_sensor_ref: owner }
          : {}),
        ...(ownerType === OwnerType.IDENTITY && owner ? { owner } : {}),
      });
      onClose();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to create key");
    }
  };

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
      <div className="bg-white rounded-lg shadow-xl max-w-2xl w-full max-h-[90vh] overflow-y-auto">
        <div className="flex items-center justify-between p-6 border-b border-gray-200">
          <h2 className="text-2xl font-bold text-gray-900">Create New Key</h2>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 transition-colors"
          >
            <X className="w-6 h-6" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="p-6 space-y-4">
          {error && (
            <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded-lg">
              {error}
            </div>
          )}

          <div>
            <label
              htmlFor="ref"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Reference <span className="text-red-500">*</span>
            </label>
            <input
              id="ref"
              type="text"
              value={ref}
              onChange={(e) => setRef(e.target.value)}
              placeholder="e.g., github_token, database_password"
              required
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <p className="mt-1 text-xs text-gray-500">
              Unique identifier (letters, numbers, _, -, .)
            </p>
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
              placeholder="e.g., GitHub API Token"
              required
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <p className="mt-1 text-xs text-gray-500">Human-readable name</p>
          </div>

          <div>
            <label
              htmlFor="format"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Value Format <span className="text-red-500">*</span>
            </label>
            <select
              id="format"
              value={format}
              onChange={(e) => setFormat(e.target.value as KeyFormat)}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              <option value="text">Text (can be encrypted)</option>
              <option value="json">JSON (can be encrypted)</option>
              <option value="yaml">YAML (can be encrypted)</option>
              <option value="number">Number (cannot be encrypted)</option>
              <option value="int">Integer (cannot be encrypted)</option>
              <option value="bool">Boolean (cannot be encrypted)</option>
            </select>
            <p className="mt-1 text-xs text-gray-500">
              {canEncrypt
                ? "This format can be encrypted for security"
                : "This format cannot be encrypted - stored as plain text"}
            </p>
          </div>

          <div>
            <label
              htmlFor="value"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Value <span className="text-red-500">*</span>
            </label>
            <textarea
              id="value"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder={
                format === "json"
                  ? '{"key": "value"}'
                  : format === "yaml"
                    ? "key: value"
                    : format === "bool"
                      ? "true or false"
                      : format === "number" || format === "int"
                        ? "123"
                        : "Enter value..."
              }
              required
              rows={format === "json" || format === "yaml" ? 6 : 3}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
            />
          </div>

          <div className="flex items-center">
            <input
              id="encrypted"
              type="checkbox"
              checked={isEncrypted}
              onChange={(e) => setEncrypted(e.target.checked)}
              disabled={!canEncrypt}
              className="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded disabled:opacity-50"
            />
            <label
              htmlFor="encrypted"
              className="ml-2 block text-sm text-gray-900"
            >
              Encrypt value (recommended for secrets)
            </label>
          </div>

          <div>
            <label
              htmlFor="ownerType"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Scope <span className="text-red-500">*</span>
            </label>
            <select
              id="ownerType"
              value={ownerType}
              onChange={(e) => setOwnerType(e.target.value as OwnerType)}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              <option value={OwnerType.SYSTEM}>System (global)</option>
              <option value={OwnerType.IDENTITY}>User (identity)</option>
              <option value={OwnerType.PACK}>Pack</option>
              <option value={OwnerType.ACTION}>Action</option>
              <option value={OwnerType.SENSOR}>Sensor</option>
            </select>
          </div>

          {ownerType !== OwnerType.SYSTEM && (
            <div>
              <label
                htmlFor="owner"
                className="block text-sm font-medium text-gray-700 mb-1"
              >
                Owner Identifier
              </label>
              <input
                id="owner"
                type="text"
                value={owner}
                onChange={(e) => setOwner(e.target.value)}
                placeholder={
                  ownerType === OwnerType.PACK
                    ? "e.g., core"
                    : ownerType === OwnerType.ACTION
                      ? "e.g., core.echo"
                      : ownerType === OwnerType.SENSOR
                        ? "e.g., core.timer_sensor"
                        : "e.g., username"
                }
                className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
              <p className="mt-1 text-xs text-gray-500">
                Optional owner reference
              </p>
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
              disabled={createKeyMutation.isPending}
              className="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {createKeyMutation.isPending ? "Creating..." : "Create Key"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
