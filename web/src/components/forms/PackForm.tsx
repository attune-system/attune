import { useState, useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { useCreatePack, useUpdatePack } from "@/hooks/usePacks";
import { PackDescriptionPatch, type PackResponse } from "@/api";
import { labelToRef } from "@/lib/format-utils";
import SchemaBuilder from "@/components/common/SchemaBuilder";
import ParamSchemaForm, {
  type ParamSchema,
} from "@/components/common/ParamSchemaForm";
import { RotateCcw } from "lucide-react";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type JsonValue = any;

/** A single property definition within a flat schema object */
interface SchemaPropertyDef {
  type?: string;
  description?: string;
  required?: boolean;
  secret?: boolean;
  default?: JsonValue;
  minimum?: number;
  maximum?: number;
  [key: string]: unknown;
}

/** The flat schema format: each key is a parameter name mapped to its definition */
type FlatSchema = Record<string, SchemaPropertyDef>;

interface PackFormProps {
  pack?: PackResponse;
  onSuccess?: () => void;
  onCancel?: () => void;
}

export default function PackForm({ pack, onSuccess, onCancel }: PackFormProps) {
  const navigate = useNavigate();
  const isEditing = !!pack;

  // Store initial/database state for reset
  const initialConfSchema = pack?.conf_schema || {};
  const initialConfig = pack?.config || {};

  // Form state
  const [ref, setRef] = useState(pack?.ref || "");
  const [label, setLabel] = useState(pack?.label || "");
  const [description, setDescription] = useState(pack?.description || "");
  const [version, setVersion] = useState(pack?.version || "1.0.0");
  const [tags, setTags] = useState(pack?.tags?.join(", ") || "");
  const [deps, setDeps] = useState(pack?.runtime_deps?.join(", ") || "");
  const [isStandard, setIsStandard] = useState(pack?.is_standard ?? false);

  const [configValues, setConfigValues] =
    useState<Record<string, JsonValue>>(initialConfig);
  const [confSchema, setConfSchema] = useState<FlatSchema>(
    initialConfSchema as FlatSchema,
  );
  const [meta, setMeta] = useState(
    pack?.meta ? JSON.stringify(pack.meta, null, 2) : "{}",
  );
  const [errors, setErrors] = useState<Record<string, string>>({});

  // Mutations
  const createPack = useCreatePack();
  const updatePack = useUpdatePack();

  // Check if schema has properties (flat format: each key is a parameter name)
  const hasSchemaProperties =
    confSchema &&
    typeof confSchema === "object" &&
    Object.keys(confSchema).length > 0;

  // Track previous confSchema to detect changes without re-running on every render
  const prevConfSchemaRef = useRef(confSchema);

  // Sync config values when schema changes (for ad-hoc packs only)
  /* eslint-disable react-hooks/set-state-in-effect -- intentional sync of dependent state */
  useEffect(() => {
    // Only sync when confSchema actually changed
    if (prevConfSchemaRef.current === confSchema) return;
    prevConfSchemaRef.current = confSchema;

    if (!isStandard && hasSchemaProperties) {
      // Get current schema property names (flat format: keys are parameter names)
      const schemaKeys = Object.keys(confSchema);

      // Create new config with only keys that exist in schema
      const syncedConfig: Record<string, JsonValue> = {};
      schemaKeys.forEach((key) => {
        if (configValues[key] !== undefined) {
          // Preserve existing value
          syncedConfig[key] = configValues[key];
        } else {
          // Use default from schema if available
          const defaultValue = confSchema[key]?.default;
          if (defaultValue !== undefined) {
            syncedConfig[key] = defaultValue;
          }
        }
      });

      // Only update if there's a difference
      const currentKeys = Object.keys(configValues).sort().join(",");
      const syncedKeys = Object.keys(syncedConfig).sort().join(",");
      if (currentKeys !== syncedKeys) {
        setConfigValues(syncedConfig);
      }
    }
  }, [confSchema, isStandard, hasSchemaProperties, configValues]);
  /* eslint-enable react-hooks/set-state-in-effect */

  const validateForm = (): boolean => {
    const newErrors: Record<string, string> = {};

    if (!label.trim()) {
      newErrors.label = "Label is required";
    }

    if (!ref.trim()) {
      newErrors.ref = "Reference is required";
    } else if (!/^[a-z0-9_-]+$/.test(ref)) {
      newErrors.ref =
        "Reference must contain only lowercase letters, numbers, hyphens, and underscores";
    }

    if (!version.trim()) {
      newErrors.version = "Version is required";
    }

    // Validate conf_schema (flat format: each value should be an object defining a parameter)
    if (confSchema && typeof confSchema === "object") {
      for (const [key, val] of Object.entries(confSchema)) {
        if (!val || typeof val !== "object" || Array.isArray(val)) {
          newErrors.confSchema = `Invalid parameter definition for "${key}" — each parameter must be an object`;
          break;
        }
      }
    }

    // Validate meta JSON
    if (meta.trim()) {
      try {
        JSON.parse(meta);
      } catch {
        newErrors.meta = "Invalid JSON format";
      }
    }

    setErrors(newErrors);
    return Object.keys(newErrors).length === 0;
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!validateForm()) {
      return;
    }

    const parsedConfSchema =
      Object.keys(confSchema || {}).length > 0 ? confSchema : {};
    const parsedMeta = meta.trim() ? JSON.parse(meta) : {};
    const tagsList = tags
      .split(",")
      .map((t) => t.trim())
      .filter((t) => t);
    const depsList: string[] = deps
      .split(",")
      .map((d: string) => d.trim())
      .filter((d: string) => d);

    try {
      if (isEditing) {
        const updateData = {
          label: label.trim(),
          description: description.trim()
            ? { op: PackDescriptionPatch.op.SET, value: description.trim() }
            : ({ op: "clear" } as unknown as PackDescriptionPatch),
          version: version.trim(),
          conf_schema: parsedConfSchema,
          config: configValues,
          meta: parsedMeta,
          tags: tagsList,
          runtime_deps: depsList,
          is_standard: isStandard,
        };
        await updatePack.mutateAsync({ ref: pack!.ref, data: updateData });
        if (onSuccess) {
          onSuccess();
        }
      } else {
        const createData = {
          ref: ref.trim(),
          label: label.trim(),
          description: description.trim() || undefined,
          version: version.trim(),
          conf_schema: parsedConfSchema,
          config: configValues,
          meta: parsedMeta,
          tags: tagsList,
          runtime_deps: depsList,
          is_standard: isStandard,
        };
        const newPackResponse = await createPack.mutateAsync(createData);
        const newPack = newPackResponse?.data;
        if (newPack?.ref) {
          navigate(`/packs/${newPack.ref}`);
          return;
        }
        if (onSuccess) {
          onSuccess();
        }
      }
    } catch (error: unknown) {
      const errMsg =
        error instanceof Error ? error.message : "Failed to save pack";
      const axiosErr = error as {
        response?: { data?: { message?: string } };
      };
      setErrors({
        submit: axiosErr?.response?.data?.message || errMsg,
      });
    }
  };

  const handleCancel = () => {
    if (onCancel) {
      onCancel();
    } else {
      navigate("/packs");
    }
  };

  const handleReset = () => {
    setConfSchema(initialConfSchema);
    setConfigValues(initialConfig);
  };

  const insertSchemaExample = (type: "api" | "database" | "webhook") => {
    let example: FlatSchema;
    switch (type) {
      case "api":
        example = {
          api_key: {
            type: "string",
            description: "API authentication key",
            required: true,
            secret: true,
          },
          endpoint: {
            type: "string",
            description: "API endpoint URL",
            default: "https://api.example.com",
          },
        };
        break;

      case "database":
        example = {
          host: {
            type: "string",
            description: "Database host",
            default: "localhost",
            required: true,
          },
          port: {
            type: "integer",
            description: "Database port",
            default: 5432,
          },
          database: {
            type: "string",
            description: "Database name",
            required: true,
          },
          username: {
            type: "string",
            description: "Database username",
            required: true,
          },
          password: {
            type: "string",
            description: "Database password",
            required: true,
            secret: true,
          },
        };
        break;

      case "webhook":
        example = {
          webhook_url: {
            type: "string",
            description: "Webhook destination URL",
            required: true,
          },
          auth_token: {
            type: "string",
            description: "Authentication token",
            secret: true,
          },
          timeout: {
            type: "integer",
            description: "Request timeout in seconds",
            minimum: 1,
            maximum: 300,
            default: 30,
          },
        };
        break;
    }

    // Update schema
    setConfSchema(example);

    // Immediately sync config values with schema defaults
    const syncedConfig: Record<string, JsonValue> = {};
    Object.entries(example).forEach(([key, propDef]) => {
      if (propDef.default !== undefined) {
        syncedConfig[key] = propDef.default;
      }
    });
    setConfigValues(syncedConfig);
  };

  return (
    <form onSubmit={handleSubmit} className="space-y-6">
      {errors.submit && (
        <div className="rounded-md bg-red-50 p-4">
          <p className="text-sm text-red-800">{errors.submit}</p>
        </div>
      )}

      {/* Basic Information */}
      <div className="bg-white shadow rounded-lg p-6 space-y-4">
        <h3 className="text-lg font-medium text-gray-900 border-b pb-2">
          Basic Information
        </h3>

        {/* Label (display name) - MOVED FIRST */}
        <div>
          <label
            htmlFor="label"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Label <span className="text-red-500">*</span>
          </label>
          <input
            type="text"
            id="label"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            onBlur={() => {
              // Auto-populate ref from label if ref is empty and not editing
              if (!isEditing && !ref.trim() && label.trim()) {
                setRef(labelToRef(label));
              }
            }}
            placeholder="e.g., My Custom Pack"
            disabled={isStandard}
            className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
              errors.label ? "border-red-500" : "border-gray-300"
            } ${isStandard ? "bg-gray-100 cursor-not-allowed" : ""}`}
          />
          {errors.label && (
            <p className="mt-1 text-sm text-red-600">{errors.label}</p>
          )}
          <p className="mt-1 text-xs text-gray-500">
            Human-readable display name
          </p>
        </div>

        {/* Ref (identifier) - MOVED AFTER LABEL */}
        <div>
          <label
            htmlFor="ref"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Reference ID <span className="text-red-500">*</span>
          </label>
          <input
            type="text"
            id="ref"
            value={ref}
            onChange={(e) => setRef(e.target.value)}
            disabled={isEditing}
            placeholder="e.g., my_custom_pack"
            className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
              errors.ref ? "border-red-500" : "border-gray-300"
            } ${isEditing ? "bg-gray-100 cursor-not-allowed" : ""}`}
          />
          {errors.ref && (
            <p className="mt-1 text-sm text-red-600">{errors.ref}</p>
          )}
          <p className="mt-1 text-xs text-gray-500">
            Unique identifier. Lowercase letters, numbers, hyphens, and
            underscores only. Auto-populated from label.
          </p>
        </div>

        {/* Description */}
        <div>
          <label
            htmlFor="description"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Description
          </label>
          <textarea
            id="description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Describe what this pack does..."
            rows={3}
            disabled={isStandard}
            className={`w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
              isStandard ? "bg-gray-100 cursor-not-allowed" : ""
            }`}
          />
        </div>

        {/* Version */}
        <div>
          <label
            htmlFor="version"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Version <span className="text-red-500">*</span>
          </label>
          <input
            type="text"
            id="version"
            value={version}
            onChange={(e) => setVersion(e.target.value)}
            placeholder="1.0.0"
            disabled={isStandard}
            className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
              errors.version ? "border-red-500" : "border-gray-300"
            } ${isStandard ? "bg-gray-100 cursor-not-allowed" : ""}`}
          />
          {errors.version && (
            <p className="mt-1 text-sm text-red-600">{errors.version}</p>
          )}
          <p className="mt-1 text-xs text-gray-500">
            Semantic version (e.g., 1.0.0)
          </p>
        </div>

        {/* Tags */}
        <div>
          <label
            htmlFor="tags"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Tags
          </label>
          <input
            type="text"
            id="tags"
            value={tags}
            onChange={(e) => setTags(e.target.value)}
            disabled={isStandard}
            className={`w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
              isStandard ? "bg-gray-100 cursor-not-allowed" : ""
            }`}
            placeholder="e.g., automation, cloud, monitoring"
          />
          <p className="mt-1 text-xs text-gray-500">
            Comma-separated tags for categorization
          </p>
        </div>

        {/* Pack Dependencies */}
        <div>
          <label
            htmlFor="deps"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Pack Dependencies
          </label>
          <input
            type="text"
            id="deps"
            value={deps}
            onChange={(e) => setDeps(e.target.value)}
            disabled={isStandard}
            className={`w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
              isStandard ? "bg-gray-100 cursor-not-allowed" : ""
            }`}
            placeholder="e.g., core, utils"
          />
          <p className="mt-1 text-xs text-gray-500">
            Comma-separated list of required pack refs (other packs this pack
            depends on)
          </p>
        </div>

        {/* Standard Pack toggle */}
        {!isEditing && (
          <div className="flex items-center">
            <input
              type="checkbox"
              id="isStandard"
              checked={isStandard}
              onChange={(e) => setIsStandard(e.target.checked)}
              className="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
            />
            <label
              htmlFor="isStandard"
              className="ml-2 block text-sm text-gray-700"
            >
              Mark as standard pack (installed/deployed)
            </label>
          </div>
        )}
      </div>

      {/* Configuration Schema */}
      <div className="bg-white shadow rounded-lg p-6 space-y-4">
        <div className="flex items-center justify-between border-b pb-2">
          <h3 className="text-lg font-medium text-gray-900">
            Configuration Schema
          </h3>
          {!isStandard && isEditing && (
            <button
              type="button"
              onClick={handleReset}
              className="flex items-center gap-2 text-sm text-gray-600 hover:text-gray-900 px-3 py-1 border border-gray-300 rounded-lg hover:bg-gray-50"
              title="Reset to database values"
            >
              <RotateCcw className="h-4 w-4" />
              Reset
            </button>
          )}
        </div>

        {!isStandard && (
          <div>
            <div className="flex items-center justify-between mb-2">
              <span className="text-xs text-gray-500">Quick examples:</span>
            </div>
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => insertSchemaExample("api")}
                className="text-xs px-2 py-1 bg-gray-100 hover:bg-gray-200 rounded"
              >
                API Example
              </button>
              <button
                type="button"
                onClick={() => insertSchemaExample("database")}
                className="text-xs px-2 py-1 bg-gray-100 hover:bg-gray-200 rounded"
              >
                Database Example
              </button>
              <button
                type="button"
                onClick={() => insertSchemaExample("webhook")}
                className="text-xs px-2 py-1 bg-gray-100 hover:bg-gray-200 rounded"
              >
                Webhook Example
              </button>
            </div>
          </div>
        )}

        <SchemaBuilder
          label="Configuration Schema"
          value={confSchema}
          onChange={setConfSchema}
          error={errors.confSchema}
          disabled={isStandard}
        />
        {isStandard ? (
          <div className="-mt-2">
            <p className="text-xs text-gray-500">
              Schema is locked for installed packs. Only configuration values
              can be edited.
            </p>
            {!hasSchemaProperties && (
              <div className="mt-3 p-3 bg-yellow-50 border border-yellow-200 rounded-lg">
                <p className="text-sm text-yellow-800">
                  <strong>Note:</strong> This installed pack has no
                  configuration schema. Configuration schemas for installed
                  packs must be updated via pack installation/upgrade and cannot
                  be edited through the web interface.
                </p>
              </div>
            )}
          </div>
        ) : (
          <p className="text-xs text-gray-500 -mt-2">
            Define the pack's configuration parameters that can be customized
          </p>
        )}

        {/* Configuration Values - Only show if schema has properties */}
        {hasSchemaProperties && (
          <div className="pt-4 border-t">
            <div className="mb-3">
              <h4 className="text-sm font-medium text-gray-900">
                Configuration Values
              </h4>
              <p className="text-xs text-gray-500 mt-1">
                Set values for the configuration parameters defined above
              </p>
            </div>
            <ParamSchemaForm
              schema={confSchema as unknown as ParamSchema}
              values={configValues}
              onChange={setConfigValues}
              errors={errors}
            />
          </div>
        )}
      </div>

      {/* Metadata */}
      <div className="bg-white shadow rounded-lg p-6 space-y-4">
        <h3 className="text-lg font-medium text-gray-900 border-b pb-2">
          Metadata
        </h3>

        <div>
          <label
            htmlFor="meta"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Metadata (JSON)
          </label>
          <textarea
            id="meta"
            value={meta}
            onChange={(e) => setMeta(e.target.value)}
            rows={6}
            disabled={isStandard}
            className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-xs ${
              errors.meta ? "border-red-500" : "border-gray-300"
            } ${isStandard ? "bg-gray-100 cursor-not-allowed" : ""}`}
            placeholder='{"author": "Your Name", "homepage": "https://..."}'
          />
          {errors.meta && (
            <p className="mt-1 text-sm text-red-600">{errors.meta}</p>
          )}
          <p className="mt-1 text-xs text-gray-500">
            Additional metadata for the pack (author, license, etc.)
          </p>
        </div>
      </div>

      {/* Form Actions */}
      <div className="flex justify-end gap-3 pt-4 border-t">
        <button
          type="button"
          onClick={handleCancel}
          className="px-4 py-2 border border-gray-300 rounded-lg text-gray-700 hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-gray-500"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={createPack.isPending || updatePack.isPending}
          className="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed focus:outline-none focus:ring-2 focus:ring-blue-500"
        >
          {createPack.isPending || updatePack.isPending
            ? "Saving..."
            : isEditing
              ? "Update Pack"
              : "Create Pack"}
        </button>
      </div>
    </form>
  );
}
