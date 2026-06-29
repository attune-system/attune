import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { OpenAPI } from "@/api";
import type { ActionResponse } from "@/api";
import MultiSelect from "@/components/common/MultiSelect";
import RetentionPolicyControls from "@/components/common/RetentionPolicyControls";
import {
  formatRetention,
  type RetentionPolicy,
} from "@/components/common/retentionPolicy";
import { useAuth } from "@/contexts/AuthContext";
import { usePermissionSets } from "@/hooks/usePermissions";
import { STANDARD_EXECUTION_ACCESS_REF } from "@/lib/permissions";
import { ChevronDown, Play, X } from "lucide-react";
import ParamSchemaForm, {
  validateParamSchema,
  extractProperties,
  type ParamSchema,
} from "@/components/common/ParamSchemaForm";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type JsonValue = any;

interface ExecuteActionModalProps {
  action: ActionResponse;
  onClose: () => void;
  initialParameters?: Record<string, JsonValue>;
  initialPermissionSetRefs?: string[];
  initialTimeoutSeconds?: number | null;
}

type PermissionOverrideMode = "default" | "none" | "custom";

/**
 * Shared modal for executing an action with a dynamic parameter form.
 *
 * Used from:
 * - ActionDetail page (Execute button)
 * - ExecutionDetailPage (Re-Run button, with initialParameters pre-filled from previous execution config)
 */
export default function ExecuteActionModal({
  action,
  onClose,
  initialParameters,
  initialPermissionSetRefs,
  initialTimeoutSeconds,
}: ExecuteActionModalProps) {
  const queryClient = useQueryClient();
  const { user } = useAuth();

  const paramSchema: ParamSchema = (action.param_schema as ParamSchema) || {};
  const paramProperties = extractProperties(paramSchema);

  // If initialParameters are provided, use them (stripping out any keys not in the schema)
  const buildInitialValues = (): Record<string, JsonValue> => {
    if (!initialParameters) return {};
    const values: Record<string, JsonValue> = {};
    // Include all initial parameters - even those not in the schema
    // so users can see exactly what was run before
    for (const [key, value] of Object.entries(initialParameters)) {
      if (value !== undefined && value !== null) {
        values[key] = value;
      }
    }
    // Also fill in defaults for any schema properties not covered
    for (const [key, param] of Object.entries(paramProperties)) {
      if (values[key] === undefined && param?.default !== undefined) {
        values[key] = param.default;
      }
    }
    return values;
  };

  const [parameters, setParameters] =
    useState<Record<string, JsonValue>>(buildInitialValues);
  const [paramErrors, setParamErrors] = useState<Record<string, string>>({});
  const [envVars, setEnvVars] = useState<Array<{ key: string; value: string }>>(
    [{ key: "", value: "" }],
  );
  const assignedPermissionSetRefs = user?.assigned_permission_set_refs ?? [];
  const isCoreAdmin = assignedPermissionSetRefs.includes("core.admin");
  const defaultPermissionSetRefs =
    action.default_execution_permission_set_refs ?? [];
  const hasDefaultPermissionSets = defaultPermissionSetRefs.length > 0;
  const canUseDefaultPermissionSets =
    hasDefaultPermissionSets &&
    (isCoreAdmin ||
      defaultPermissionSetRefs.every(
        (ref) =>
          ref === STANDARD_EXECUTION_ACCESS_REF ||
          assignedPermissionSetRefs.includes(ref),
      ));
  const initialPermissionMode: PermissionOverrideMode =
    initialPermissionSetRefs === undefined
      ? canUseDefaultPermissionSets
        ? "default"
        : "none"
      : initialPermissionSetRefs.length > 0
        ? "custom"
        : "none";
  const [permissionMode, setPermissionMode] = useState<PermissionOverrideMode>(
    initialPermissionMode,
  );
  const [selectedPermissionSetRefs, setSelectedPermissionSetRefs] = useState<
    string[]
  >(initialPermissionSetRefs ?? []);
  const [isTokenAccessOpen, setIsTokenAccessOpen] = useState(false);
  const [isArtifactRetentionOpen, setIsArtifactRetentionOpen] = useState(false);
  const [overrideArtifactRetention, setOverrideArtifactRetention] =
    useState(false);
  const [artifactRetentionPolicy, setArtifactRetentionPolicy] =
    useState<RetentionPolicy>(
      (action.artifact_retention_policy as RetentionPolicy | undefined) ??
        "versions",
    );
  const [artifactRetentionLimit, setArtifactRetentionLimit] = useState(
    action.artifact_retention_limit ?? 5,
  );
  const [overrideTimeout, setOverrideTimeout] = useState(
    initialTimeoutSeconds != null,
  );
  const [timeoutSeconds, setTimeoutSeconds] = useState<number>(
    initialTimeoutSeconds ?? action.timeout_seconds ?? 600,
  );

  const permissionSets = usePermissionSets(null, { enabled: isCoreAdmin });
  const allPermissionSetRefs =
    permissionSets.data?.map((permissionSet) => permissionSet.ref) ?? [];
  const selectablePermissionSetRefs = Array.from(
    new Set(
      isCoreAdmin
        ? [
            STANDARD_EXECUTION_ACCESS_REF,
            ...allPermissionSetRefs,
            ...selectedPermissionSetRefs,
          ]
        : [
            STANDARD_EXECUTION_ACCESS_REF,
            ...assignedPermissionSetRefs,
            ...selectedPermissionSetRefs.filter(
              (ref) => ref === STANDARD_EXECUTION_ACCESS_REF,
            ),
          ],
    ),
  ).sort((a, b) => a.localeCompare(b));
  const selectablePermissionSetOptions = selectablePermissionSetRefs.map(
    (ref) => ({
      value: ref,
      label:
        ref === STANDARD_EXECUTION_ACCESS_REF
          ? "standard (action/pack-scoped keys and artifacts)"
          : ref,
    }),
  );
  const allowedSelectedPermissionSetRefs = selectedPermissionSetRefs.filter(
    (ref) => selectablePermissionSetRefs.includes(ref),
  );

  const executeAction = useMutation({
    mutationFn: async (params: {
      parameters: Record<string, JsonValue>;
      envVars: Array<{ key: string; value: string }>;
      permissionSetRefs?: string[];
      artifactRetentionPolicy?: RetentionPolicy;
      artifactRetentionLimit?: number;
      timeoutSeconds?: number;
    }) => {
      const token =
        typeof OpenAPI.TOKEN === "function"
          ? await OpenAPI.TOKEN({} as Parameters<typeof OpenAPI.TOKEN>[0])
          : OpenAPI.TOKEN;

      const response = await fetch(
        `${OpenAPI.BASE}/api/v1/executions/execute`,
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${token}`,
          },
          body: JSON.stringify({
            action_ref: action.ref,
            parameters: params.parameters,
            env_vars: params.envVars
              .filter((ev) => ev.key.trim() !== "")
              .reduce(
                (acc, ev) => {
                  acc[ev.key] = ev.value;
                  return acc;
                },
                {} as Record<string, string>,
              ),
            ...(params.permissionSetRefs === undefined
              ? {}
              : { permission_set_refs: params.permissionSetRefs }),
            ...(params.artifactRetentionPolicy && params.artifactRetentionLimit
              ? {
                  artifact_retention_policy: params.artifactRetentionPolicy,
                  artifact_retention_limit: params.artifactRetentionLimit,
                }
              : {}),
            ...(params.timeoutSeconds && params.timeoutSeconds > 0
              ? { timeout_seconds: params.timeoutSeconds }
              : {}),
          }),
        },
      );

      if (!response.ok) {
        const error = await response.json();
        throw new Error(error.message || "Failed to execute action");
      }

      return response.json();
    },
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ["executions"] });
      onClose();
      if (data?.data?.id) {
        window.location.href = `/executions/${data.data.id}`;
      }
    },
  });

  const validateForm = (): boolean => {
    const errors = validateParamSchema(paramSchema, parameters);
    setParamErrors(errors);
    return Object.keys(errors).length === 0;
  };

  const handleExecute = async () => {
    if (!validateForm()) {
      return;
    }

    try {
      const permissionSetRefs =
        permissionMode === "default"
          ? undefined
          : permissionMode === "none"
            ? []
            : selectedPermissionSetRefs.filter((ref) =>
                selectablePermissionSetRefs.includes(ref),
              );

      await executeAction.mutateAsync({
        parameters,
        envVars,
        permissionSetRefs,
        artifactRetentionPolicy: overrideArtifactRetention
          ? artifactRetentionPolicy
          : undefined,
        artifactRetentionLimit: overrideArtifactRetention
          ? artifactRetentionLimit
          : undefined,
        timeoutSeconds: overrideTimeout ? timeoutSeconds : undefined,
      });
    } catch (err) {
      console.error("Failed to execute action:", err);
    }
  };

  const addEnvVar = () => {
    setEnvVars([...envVars, { key: "", value: "" }]);
  };

  const removeEnvVar = (index: number) => {
    if (envVars.length > 1) {
      setEnvVars(envVars.filter((_, i) => i !== index));
    }
  };

  const updateEnvVar = (
    index: number,
    field: "key" | "value",
    value: string,
  ) => {
    const updated = [...envVars];
    updated[index][field] = value;
    setEnvVars(updated);
  };

  const permissionSummary = (() => {
    if (permissionMode === "default") {
      return `Action default: ${defaultPermissionSetRefs.join(", ")}`;
    }

    if (permissionMode === "none") {
      return "No execution API token";
    }

    if (allowedSelectedPermissionSetRefs.length === 0) {
      return "Custom: no permission sets selected";
    }

    return `Custom: ${allowedSelectedPermissionSetRefs.join(", ")}`;
  })();
  const artifactRetentionSummary = overrideArtifactRetention
    ? `Custom: ${formatRetention(artifactRetentionPolicy, artifactRetentionLimit)}`
    : `Default: ${formatRetention(
        action.artifact_retention_policy as RetentionPolicy | null | undefined,
        action.artifact_retention_limit,
        "versions / 5",
      )}`;

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
      <div className="bg-white rounded-lg p-6 max-w-2xl w-full max-h-[90vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-xl font-bold">
            {initialParameters ? "Re-Run Action" : "Execute Action"}
          </h3>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600"
          >
            <X className="h-6 w-6" />
          </button>
        </div>

        <div className="mb-4">
          <p className="text-sm text-gray-600">
            Action:{" "}
            <span className="font-mono text-gray-900">{action.ref}</span>
          </p>
          {action.description && (
            <p className="text-sm text-gray-600 mt-1">{action.description}</p>
          )}
          {initialParameters && (
            <p className="text-xs text-blue-600 mt-2 bg-blue-50 px-3 py-1.5 rounded">
              Parameters pre-filled from previous execution. Modify as needed
              before re-running.
            </p>
          )}
        </div>

        {executeAction.error && (
          <div className="mb-4 p-3 bg-red-50 border border-red-200 text-red-700 rounded-lg text-sm">
            {(executeAction.error as Error).message}
          </div>
        )}

        <div className="mb-6">
          <h4 className="text-sm font-semibold text-gray-700 mb-2">
            Parameters
          </h4>
          <ParamSchemaForm
            schema={paramSchema}
            values={parameters}
            onChange={setParameters}
            errors={paramErrors}
          />
        </div>

        <div className="mb-6">
          <h4 className="text-sm font-semibold text-gray-700 mb-2">
            Environment Variables
          </h4>
          <p className="text-xs text-gray-500 mb-3">
            Optional environment variables for this execution (e.g., DEBUG,
            LOG_LEVEL)
          </p>
          <div className="space-y-2">
            {envVars.map((envVar, index) => (
              <div key={index} className="flex gap-2 items-start">
                <input
                  type="text"
                  placeholder="Key"
                  value={envVar.key}
                  onChange={(e) => updateEnvVar(index, "key", e.target.value)}
                  className="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                />
                <input
                  type="text"
                  placeholder="Value"
                  value={envVar.value}
                  onChange={(e) => updateEnvVar(index, "value", e.target.value)}
                  className="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                />
                <button
                  type="button"
                  onClick={() => removeEnvVar(index)}
                  disabled={envVars.length === 1}
                  className="px-3 py-2 text-red-600 hover:text-red-700 disabled:text-gray-300 disabled:cursor-not-allowed"
                  title="Remove"
                >
                  <X className="h-5 w-5" />
                </button>
              </div>
            ))}
          </div>
          <button
            type="button"
            onClick={addEnvVar}
            className="mt-2 text-sm text-blue-600 hover:text-blue-700"
          >
            + Add Environment Variable
          </button>
        </div>

        <div className="mb-6 rounded-lg border border-gray-200">
          <button
            type="button"
            onClick={() => setIsTokenAccessOpen((open) => !open)}
            className="flex w-full items-center justify-between gap-3 px-3 py-3 text-left hover:bg-gray-50"
            aria-expanded={isTokenAccessOpen}
          >
            <span>
              <span className="block text-sm font-semibold text-gray-700">
                Execution Token Access
              </span>
              <span className="mt-1 block truncate text-xs text-gray-500">
                {permissionSummary}
              </span>
            </span>
            <ChevronDown
              className={`h-4 w-4 flex-shrink-0 text-gray-400 transition-transform ${
                isTokenAccessOpen ? "rotate-180" : ""
              }`}
            />
          </button>
          {isTokenAccessOpen && (
            <div className="space-y-3 border-t border-gray-200 p-3">
              <p className="text-xs text-gray-500">
                Controls which Attune API permissions are granted to this
                execution. Without permission sets, no execution API token is
                issued.
              </p>
              {hasDefaultPermissionSets && (
                <label className="flex items-start gap-2 text-sm">
                  <input
                    type="radio"
                    name="permission-mode"
                    checked={permissionMode === "default"}
                    onChange={() => setPermissionMode("default")}
                    disabled={!canUseDefaultPermissionSets}
                    className="mt-1"
                  />
                  <span>
                    <span className="font-medium text-gray-900">
                      Use action default
                    </span>
                    <span className="block text-xs text-gray-500">
                      {canUseDefaultPermissionSets
                        ? defaultPermissionSetRefs.join(", ")
                        : `${defaultPermissionSetRefs.join(", ")} (not assignable by your account)`}
                    </span>
                  </span>
                </label>
              )}
              <label className="flex items-start gap-2 text-sm">
                <input
                  type="radio"
                  name="permission-mode"
                  checked={permissionMode === "none"}
                  onChange={() => setPermissionMode("none")}
                  className="mt-1"
                />
                <span>
                  <span className="font-medium text-gray-900">
                    No execution API token
                  </span>
                  <span className="block text-xs text-gray-500">
                    {hasDefaultPermissionSets
                      ? "The worker will omit ATTUNE_API_TOKEN for this execution."
                      : "This action has no default token access, so the worker will omit ATTUNE_API_TOKEN."}
                  </span>
                </span>
              </label>
              <div>
                <label className="flex items-start gap-2 text-sm">
                  <input
                    type="radio"
                    name="permission-mode"
                    checked={permissionMode === "custom"}
                    onChange={() => setPermissionMode("custom")}
                    className="mt-1"
                  />
                  <span>
                    <span className="font-medium text-gray-900">
                      Specify permission sets
                    </span>
                    <span className="block text-xs text-gray-500">
                      {isCoreAdmin
                        ? "core.admin can assign any permission set."
                        : "You can assign standard access and permission sets assigned to you."}
                    </span>
                  </span>
                </label>
                {permissionMode === "custom" && (
                  <div className="mt-3 pl-6">
                    {isCoreAdmin && permissionSets.isLoading ? (
                      <p className="text-xs text-gray-500">
                        Loading permission sets...
                      </p>
                    ) : selectablePermissionSetRefs.length > 0 ? (
                      <MultiSelect
                        options={selectablePermissionSetOptions}
                        value={allowedSelectedPermissionSetRefs}
                        onChange={(refs) =>
                          setSelectedPermissionSetRefs(
                            refs.sort((a, b) => a.localeCompare(b)),
                          )
                        }
                        placeholder="Search and select permission sets..."
                      />
                    ) : (
                      <p className="text-xs text-gray-500">
                        No assignable permission sets are available.
                      </p>
                    )}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>

        <div className="mb-6 rounded-lg border border-gray-200">
          <button
            type="button"
            onClick={() => setIsArtifactRetentionOpen((open) => !open)}
            className="flex w-full items-center justify-between gap-3 px-3 py-3 text-left hover:bg-gray-50"
            aria-expanded={isArtifactRetentionOpen}
          >
            <span>
              <span className="block text-sm font-semibold text-gray-700">
                Artifact Retention
              </span>
              <span className="mt-1 block truncate text-xs text-gray-500">
                {artifactRetentionSummary}
              </span>
            </span>
            <ChevronDown
              className={`h-4 w-4 flex-shrink-0 text-gray-400 transition-transform ${
                isArtifactRetentionOpen ? "rotate-180" : ""
              }`}
            />
          </button>
          {isArtifactRetentionOpen && (
            <div className="border-t border-gray-200 p-3">
              <RetentionPolicyControls
                title="Non-log artifact retention"
                description="Applies only to artifacts this execution creates. Stdout/stderr logs use the action log-retention default."
                policy={
                  overrideArtifactRetention ? artifactRetentionPolicy : null
                }
                limit={
                  overrideArtifactRetention ? artifactRetentionLimit : null
                }
                inheritedLabel={`Use action default: ${formatRetention(
                  action.artifact_retention_policy as
                    RetentionPolicy | null | undefined,
                  action.artifact_retention_limit,
                  "versions / 5",
                )}`}
                onChange={({ policy, limit }) => {
                  setOverrideArtifactRetention(Boolean(policy || limit));
                  if (policy) setArtifactRetentionPolicy(policy);
                  if (limit) setArtifactRetentionLimit(limit);
                }}
              />
            </div>
          )}
        </div>

        <div className="mb-6 rounded-lg border border-gray-200 p-3">
          <label className="flex items-center gap-2 text-sm font-semibold text-gray-700">
            <input
              type="checkbox"
              checked={overrideTimeout}
              onChange={(e) => setOverrideTimeout(e.target.checked)}
            />
            Override execution timeout
          </label>
          <p className="mt-1 text-xs text-gray-500">
            {overrideTimeout
              ? "This execution will be terminated after the timeout below."
              : `Use action default: ${
                  action.timeout_seconds
                    ? `${action.timeout_seconds}s`
                    : "platform default"
                }`}
          </p>
          {overrideTimeout && (
            <div className="mt-2 flex items-center gap-2">
              <input
                type="number"
                min={1}
                value={timeoutSeconds}
                onChange={(e) => setTimeoutSeconds(Number(e.target.value))}
                className="w-32 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-green-500 focus:outline-none focus:ring-1 focus:ring-green-500"
              />
              <span className="text-xs text-gray-500">seconds</span>
            </div>
          )}
        </div>

        <div className="flex justify-end gap-3">
          <button
            onClick={onClose}
            disabled={executeAction.isPending}
            className="px-4 py-2 bg-gray-200 rounded hover:bg-gray-300 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            onClick={handleExecute}
            disabled={executeAction.isPending}
            className="px-4 py-2 bg-green-600 text-white rounded hover:bg-green-700 disabled:opacity-50 flex items-center gap-2"
          >
            {executeAction.isPending ? (
              <>
                <div className="animate-spin rounded-full h-4 w-4 border-b-2 border-white" />
                Executing...
              </>
            ) : (
              <>
                <Play className="h-4 w-4" />
                {initialParameters ? "Re-Run" : "Execute"}
              </>
            )}
          </button>
        </div>
      </div>
    </div>
  );
}
