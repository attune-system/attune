import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { usePacks } from "@/hooks/usePacks";
import { useCreateRuntime, useUpdateRuntime } from "@/hooks/useRuntimes";
import {
  NullableJsonPatch,
  NullableStringPatch,
  type PackSummary,
  type RuntimeResponse,
} from "@/api";

interface RuntimeFormProps {
  initialData?: RuntimeResponse;
  isEditing?: boolean;
  onCancel?: () => void;
}

type JsonValue =
  string | number | boolean | null | { [key: string]: JsonValue } | JsonValue[];
type JsonObject = { [key: string]: JsonValue };
type NonNullJsonValue = Exclude<JsonValue, null>;

function prettyJson(value: unknown): string {
  return JSON.stringify(value ?? {}, null, 2);
}

function validateObjectJson(label: string, raw: string): JsonObject {
  if (!raw.trim()) {
    throw new Error(`${label} is required`);
  }

  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error(`${label} must be a JSON object`);
    }
    return parsed as JsonObject;
  } catch (error) {
    if (error instanceof Error) {
      throw error;
    }
    throw new Error(`${label} must be valid JSON`);
  }
}

function validateJsonValue(
  label: string,
  raw: string,
  required = true,
): NonNullJsonValue | null {
  if (!raw.trim()) {
    if (required) {
      throw new Error(`${label} is required`);
    }
    return null;
  }

  try {
    return JSON.parse(raw) as NonNullJsonValue;
  } catch {
    throw new Error(`${label} must be valid JSON`);
  }
}

export default function RuntimeForm({
  initialData,
  isEditing = false,
  onCancel,
}: RuntimeFormProps) {
  const navigate = useNavigate();
  const { data: packsData } = usePacks({ page: 1, pageSize: 200 });
  const packs = useMemo(() => packsData?.items || [], [packsData?.items]);
  const createRuntime = useCreateRuntime();
  const updateRuntime = useUpdateRuntime();

  const [ref, setRef] = useState(() => initialData?.ref ?? "");
  const [packRef, setPackRef] = useState(() => initialData?.pack_ref ?? "");
  const [name, setName] = useState(() => initialData?.name ?? "");
  const [description, setDescription] = useState(
    () => initialData?.description ?? "",
  );
  const [distributions, setDistributions] = useState(() =>
    prettyJson(initialData?.distributions ?? {}),
  );
  const [installation, setInstallation] = useState(() =>
    initialData?.installation == null
      ? ""
      : prettyJson(initialData.installation),
  );
  const [executionConfig, setExecutionConfig] = useState(() =>
    prettyJson(initialData?.execution_config ?? {}),
  );
  const [errors, setErrors] = useState<Record<string, string>>({});

  const canEditRef = !isEditing;
  const isSubmitting = createRuntime.isPending || updateRuntime.isPending;
  const selectedPackExists =
    !packRef || packs.some((pack: PackSummary) => pack.ref === packRef);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    const newErrors: Record<string, string> = {};

    if (!ref.trim()) {
      newErrors.ref = "Reference is required";
    }
    if (!name.trim()) {
      newErrors.name = "Name is required";
    }
    if (packRef && !selectedPackExists) {
      newErrors.pack_ref = "Selected pack does not exist";
    }

    let parsedDistributions: JsonObject | undefined;
    let parsedExecutionConfig: JsonObject | undefined;
    let parsedInstallation: NonNullJsonValue | null = null;

    try {
      parsedDistributions = validateObjectJson("Distributions", distributions);
    } catch (error) {
      newErrors.distributions =
        error instanceof Error ? error.message : "Invalid distributions JSON";
    }

    try {
      parsedExecutionConfig = validateObjectJson(
        "Execution config",
        executionConfig,
      );
    } catch (error) {
      newErrors.execution_config =
        error instanceof Error
          ? error.message
          : "Invalid execution config JSON";
    }

    try {
      parsedInstallation = validateJsonValue(
        "Installation",
        installation,
        false,
      );
    } catch (error) {
      newErrors.installation =
        error instanceof Error ? error.message : "Invalid installation JSON";
    }

    if (Object.keys(newErrors).length > 0) {
      setErrors(newErrors);
      return;
    }

    try {
      if (isEditing && initialData) {
        const installationPatch =
          installation.trim().length > 0 && parsedInstallation !== null
            ? { op: NullableJsonPatch.op.SET, value: parsedInstallation }
            : null;

        await updateRuntime.mutateAsync({
          ref: initialData.ref,
          data: {
            description: description.trim()
              ? { op: NullableStringPatch.op.SET, value: description.trim() }
              : null,
            name: name.trim(),
            distributions: parsedDistributions,
            installation: installationPatch,
            execution_config: parsedExecutionConfig,
          },
        });
        navigate(`/runtimes/${encodeURIComponent(initialData.ref)}`);
      } else {
        const response = await createRuntime.mutateAsync({
          ref: ref.trim(),
          pack_ref: packRef.trim() || null,
          description: description.trim() || null,
          name: name.trim(),
          distributions: parsedDistributions,
          installation:
            installation.trim().length > 0 ? parsedInstallation : null,
          execution_config: parsedExecutionConfig,
        });
        navigate(`/runtimes/${encodeURIComponent(response.data.ref)}`);
      }
    } catch (error: unknown) {
      const axiosErr = error as { response?: { data?: { message?: string } } };
      setErrors({
        submit:
          axiosErr?.response?.data?.message ||
          (error instanceof Error ? error.message : "Failed to save runtime"),
      });
    }
  };

  return (
    <form onSubmit={handleSubmit} className="space-y-6 p-6 max-w-5xl">
      <div>
        <h2 className="text-3xl font-bold text-gray-900">
          {isEditing ? "Edit Runtime" : "Create Runtime"}
        </h2>
        <p className="mt-2 text-sm text-gray-600">
          Configure the metadata and execution contract for a runtime.
        </p>
      </div>

      {errors.submit && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700">
          {errors.submit}
        </div>
      )}

      <div className="grid gap-6 lg:grid-cols-2">
        <div className="bg-white rounded-lg shadow p-6 space-y-4">
          <h3 className="text-lg font-semibold text-gray-900">Basics</h3>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Runtime Ref
            </label>
            <input
              value={ref}
              onChange={(e) => setRef(e.target.value)}
              disabled={!canEditRef}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm disabled:bg-gray-100"
              placeholder="core.python"
            />
            {errors.ref && (
              <p className="mt-1 text-sm text-red-600">{errors.ref}</p>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Name
            </label>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Python"
            />
            {errors.name && (
              <p className="mt-1 text-sm text-red-600">{errors.name}</p>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Pack
            </label>
            <select
              value={packRef}
              onChange={(e) => setPackRef(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="">No pack</option>
              {packs.map((pack: PackSummary) => (
                <option key={pack.id} value={pack.ref}>
                  {pack.ref}
                </option>
              ))}
            </select>
            {errors.pack_ref && (
              <p className="mt-1 text-sm text-red-600">{errors.pack_ref}</p>
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
              placeholder="Optional description"
            />
          </div>
        </div>

        <div className="bg-amber-50 border border-amber-200 rounded-lg p-6">
          <h3 className="text-lg font-semibold text-amber-900">
            Patch Semantics
          </h3>
          <p className="mt-2 text-sm text-amber-800">
            Saving an existing runtime sends explicit patch operations for
            nullable fields. Blank description or installation clears the stored
            value.
          </p>
        </div>
      </div>

      <JsonField
        label="Distributions"
        value={distributions}
        onChange={setDistributions}
        error={errors.distributions}
      />
      <JsonField
        label="Installation"
        value={installation}
        onChange={setInstallation}
        error={errors.installation}
        placeholder='Leave blank to clear or omit, e.g. { "method": "system" }'
      />
      <JsonField
        label="Execution Config"
        value={executionConfig}
        onChange={setExecutionConfig}
        error={errors.execution_config}
      />

      <div className="flex items-center gap-3">
        <button
          type="submit"
          disabled={isSubmitting}
          className="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50"
        >
          {isSubmitting
            ? "Saving..."
            : isEditing
              ? "Save Runtime"
              : "Create Runtime"}
        </button>
        <button
          type="button"
          onClick={() => (onCancel ? onCancel() : navigate("/runtimes"))}
          className="px-4 py-2 border border-gray-300 rounded-lg text-gray-700 hover:bg-gray-50"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

function JsonField({
  label,
  value,
  onChange,
  error,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
  placeholder?: string;
}) {
  return (
    <div className="bg-white rounded-lg shadow p-6">
      <label className="block text-sm font-medium text-gray-700 mb-2">
        {label}
      </label>
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={12}
        className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm font-mono"
        placeholder={placeholder ?? "{\n  \n}"}
      />
      {error && <p className="mt-2 text-sm text-red-600">{error}</p>}
    </div>
  );
}
