import { useState, useCallback } from "react";
import { Play, X, ExternalLink } from "lucide-react";
import ParamSchemaForm, {
  validateParamSchema,
  extractProperties,
  type ParamSchema,
} from "@/components/common/ParamSchemaForm";
import { useRequestExecution } from "@/hooks/useExecutions";
import { safeErrorSummary } from "@/lib/safe-error-summary";

interface RunWorkflowModalProps {
  /** The workflow's action ref (e.g., "examples.hello_workflow") */
  actionRef: string;
  /** The workflow's param_schema in flat format */
  paramSchema: ParamSchema;
  /** Called before executing — should save the workflow. Return true if save succeeded. */
  onSave: () => Promise<boolean>;
  /** Called when the modal is closed (cancel or after successful execution) */
  onClose: () => void;
  /** Optional label for display */
  label?: string;
}

/**
 * Modal for running a workflow with optional parameter overrides.
 *
 * Shown from the workflow builder's "Run" button when the workflow has
 * parameters defined.  Displays a ParamSchemaForm pre-populated with
 * default values, saves the workflow first, then creates an execution
 * and opens the execution detail page in a new tab.
 */
export default function RunWorkflowModal({
  actionRef,
  paramSchema,
  onSave,
  onClose,
  label,
}: RunWorkflowModalProps) {
  const requestExecution = useRequestExecution();

  const paramProperties = extractProperties(paramSchema);

  // Build initial values from schema defaults
  const buildInitialValues = (): Record<string, unknown> => {
    const values: Record<string, unknown> = {};
    for (const [key, prop] of Object.entries(paramProperties)) {
      if (prop?.default !== undefined) {
        values[key] = prop.default;
      }
    }
    return values;
  };

  const [parameters, setParameters] =
    useState<Record<string, unknown>>(buildInitialValues);
  const [paramErrors, setParamErrors] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [previewDigest, setPreviewDigest] = useState<string | undefined>();
  const [acceptedExecutionId, setAcceptedExecutionId] = useState<number | null>(
    null,
  );
  const [phase, setPhase] = useState<"idle" | "saving" | "executing">("idle");

  const isSubmitting = phase !== "idle";

  const handleExecute = useCallback(async () => {
    // Validate parameters against schema
    const errors = validateParamSchema(paramSchema, parameters);
    setParamErrors(errors);
    if (Object.keys(errors).length > 0) return;

    setError(null);

    // Phase 1: Save the workflow
    setPhase("saving");
    try {
      const saved = await onSave();
      if (!saved) {
        setPhase("idle");
        return; // save failed — error shown by parent
      }
    } catch {
      setError("Failed to save workflow");
      setPhase("idle");
      return;
    }

    // Phase 2: Execute
    setPhase("executing");
    try {
      // Only missing values receive defaults. Preserve explicit empty strings,
      // null, false, and zero across the admission boundary.
      const cleanedParams: Record<string, unknown> = {};
      for (const [key, value] of Object.entries(parameters)) {
        if (value !== undefined) {
          cleanedParams[key] = value;
        }
      }

      const response = await requestExecution.mutateAsync({
        actionRef,
        parameters: cleanedParams,
      });
      const executionId = response.data.id;
      const binding: unknown = response.data.schema_binding;
      const acceptedDigest =
        typeof binding === "object" &&
        binding !== null &&
        "input_digest" in binding
          ? binding.input_digest
          : undefined;

      // Open execution in new tab and close the modal
      window.open(`/executions/${executionId}`, "_blank");
      if (
        previewDigest !== undefined &&
        typeof acceptedDigest === "string" &&
        previewDigest !== acceptedDigest
      ) {
        setAcceptedExecutionId(executionId);
        setPhase("idle");
      } else {
        onClose();
      }
    } catch (err: unknown) {
      const summary = safeErrorSummary(err);
      setError(
        summary.status === undefined
          ? "Failed to start execution"
          : `Failed to start execution. HTTP ${summary.status}.`,
      );
      setPhase("idle");
    }
  }, [
    paramSchema,
    parameters,
    onSave,
    actionRef,
    requestExecution,
    onClose,
    previewDigest,
  ]);

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50 p-4">
      <div className="bg-white rounded-lg shadow-xl border border-gray-200 max-w-lg w-full max-h-[85vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-4 border-b border-gray-200 flex-shrink-0">
          <div className="min-w-0">
            <h3 className="text-base font-semibold text-gray-900 truncate">
              Run Workflow
            </h3>
            <p className="text-xs text-gray-500 font-mono mt-0.5 truncate">
              {label || actionRef}
            </p>
          </div>
          <button
            onClick={onClose}
            disabled={isSubmitting}
            className="p-1 rounded hover:bg-gray-100 text-gray-400 hover:text-gray-600 transition-colors disabled:opacity-50 flex-shrink-0"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-4">
          {acceptedExecutionId !== null && (
            <div
              role="status"
              className="mb-4 p-3 bg-amber-50 border border-amber-200 text-amber-800 rounded-lg text-sm"
            >
              The accepted input contract differs from the form preview.
              Execution {acceptedExecutionId} uses the server-selected contract.
            </div>
          )}
          {error && (
            <div className="mb-4 p-3 bg-red-50 border border-red-200 text-red-700 rounded-lg text-sm">
              {error}
            </div>
          )}

          <div>
            <h4 className="text-sm font-medium text-gray-700 mb-1.5">
              Parameters
            </h4>
            <p className="text-xs text-gray-500 mb-3">
              Override default values or leave as-is to use the schema defaults.
            </p>
            <ParamSchemaForm
              schema={paramSchema}
              values={parameters}
              onChange={setParameters}
              errors={paramErrors}
              onPreviewDigest={setPreviewDigest}
              disabled={isSubmitting || acceptedExecutionId !== null}
            />
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-2.5 px-5 py-3 border-t border-gray-200 bg-gray-50 rounded-b-lg flex-shrink-0">
          <button
            onClick={onClose}
            disabled={isSubmitting}
            className="px-4 py-1.5 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded hover:bg-gray-50 disabled:opacity-50 transition-colors"
          >
            {acceptedExecutionId === null ? "Cancel" : "Close"}
          </button>
          <button
            onClick={handleExecute}
            disabled={isSubmitting || acceptedExecutionId !== null}
            className="flex items-center gap-1.5 px-4 py-1.5 text-sm font-medium text-white bg-green-600 rounded hover:bg-green-700 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shadow-sm"
          >
            {phase === "saving" ? (
              <>
                <div className="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin" />
                Saving…
              </>
            ) : phase === "executing" ? (
              <>
                <div className="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin" />
                Executing…
              </>
            ) : (
              <>
                <Play className="w-4 h-4" />
                Run
                <ExternalLink className="w-3 h-3 opacity-60" />
              </>
            )}
          </button>
        </div>
      </div>
    </div>
  );
}
