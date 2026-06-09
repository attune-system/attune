import { useState } from "react";
import {
  Pencil,
  Plus,
  X,
  LogIn,
  LogOut,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";
import SchemaBuilder from "@/components/common/SchemaBuilder";
import type {
  CancellationPolicy,
  ParamDefinition,
  ReferenceVisibility,
} from "@/types/workflow";
import { CANCELLATION_POLICY_LABELS } from "@/types/workflow";

interface WorkflowInputsPanelProps {
  label: string;
  version: string;
  description: string;
  tags: string[];
  referenceVisibility: ReferenceVisibility;
  referenceAllowedPackRefs: string[];
  cancellationPolicy: CancellationPolicy;
  parameters: Record<string, ParamDefinition>;
  output: Record<string, ParamDefinition>;
  outputMap: Record<string, string>;
  onLabelChange: (label: string) => void;
  onVersionChange: (version: string) => void;
  onDescriptionChange: (description: string) => void;
  onTagsChange: (tags: string[]) => void;
  onReferenceVisibilityChange: (visibility: ReferenceVisibility) => void;
  onReferenceAllowedPackRefsChange: (packRefs: string[]) => void;
  onCancellationPolicyChange: (policy: CancellationPolicy) => void;
  onParametersChange: (parameters: Record<string, ParamDefinition>) => void;
  onOutputChange: (output: Record<string, ParamDefinition>) => void;
  onOutputMapChange: (outputMap: Record<string, string>) => void;
}

type ModalTarget = "parameters" | "output" | null;

interface OutputRow {
  key: string;
  type: string;
  required: boolean;
  secret: boolean;
  description: string;
  expression: string;
}

const OUTPUT_TYPES = [
  "string",
  "number",
  "integer",
  "boolean",
  "object",
  "array",
  "any",
];

function buildOutputRows(
  schema: Record<string, ParamDefinition>,
  outputMap: Record<string, string>,
): OutputRow[] {
  const seen = new Set<string>();
  const rows: OutputRow[] = [];

  for (const [key, def] of Object.entries(schema)) {
    seen.add(key);
    rows.push({
      key,
      type: typeof def.type === "string" ? def.type : "string",
      required: !!def.required,
      secret: !!def.secret,
      description:
        typeof def.description === "string" ? def.description : "",
      expression: outputMap[key] ?? "",
    });
  }
  // Mapped fields without a schema entry still need to round-trip.
  for (const [key, expr] of Object.entries(outputMap)) {
    if (!seen.has(key)) {
      rows.push({
        key,
        type: "string",
        required: false,
        secret: false,
        description: "",
        expression: expr,
      });
    }
  }
  return rows;
}

function rowsToSchemaAndMap(rows: OutputRow[]): {
  schema: Record<string, ParamDefinition>;
  outputMap: Record<string, string>;
} {
  const schema: Record<string, ParamDefinition> = {};
  const outputMap: Record<string, string> = {};
  for (const row of rows) {
    const key = row.key.trim();
    if (!key) continue;
    const def: ParamDefinition = { type: row.type || "string" };
    if (row.required) def.required = true;
    if (row.secret) def.secret = true;
    if (row.description.trim()) def.description = row.description.trim();
    schema[key] = def;
    if (row.expression.length > 0) {
      outputMap[key] = row.expression;
    }
  }
  return { schema, outputMap };
}

function ParamSummaryList({
  schema,
  emptyMessage,
  onEdit,
}: {
  schema: Record<string, ParamDefinition>;
  emptyMessage: string;
  onEdit: () => void;
}) {
  const entries = Object.entries(schema);

  if (entries.length === 0) {
    return (
      <button
        onClick={onEdit}
        className="w-full px-3 py-4 border-2 border-dashed border-gray-200 rounded-lg text-center hover:border-blue-300 hover:bg-blue-50/30 transition-colors group"
      >
        <Plus className="w-4 h-4 text-gray-300 group-hover:text-blue-400 mx-auto mb-1" />
        <span className="text-[11px] text-gray-400 group-hover:text-blue-500">
          {emptyMessage}
        </span>
      </button>
    );
  }

  return (
    <div className="space-y-1">
      {entries.map(([name, def]) => (
        <div
          key={name}
          className="flex items-center gap-1.5 px-2 py-1.5 bg-white border border-gray-150 rounded-md"
        >
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-1.5">
              <span className="font-mono text-[11px] font-medium text-gray-800 truncate">
                {name}
              </span>
              {def.required && (
                <span className="text-[9px] font-semibold text-red-500">*</span>
              )}
            </div>
            <div className="flex items-center gap-1.5">
              <span className="text-[10px] text-blue-600/70">{def.type}</span>
              {def.secret && (
                <span className="text-[9px] text-amber-500">secret</span>
              )}
              {def.default !== undefined && (
                <span
                  className="text-[9px] text-gray-400 truncate max-w-[80px]"
                  title={`default: ${JSON.stringify(def.default)}`}
                >
                  = {JSON.stringify(def.default)}
                </span>
              )}
            </div>
          </div>
        </div>
      ))}
      <button
        onClick={onEdit}
        className="flex items-center gap-1 px-2 py-1 text-[11px] text-gray-400 hover:text-blue-600 transition-colors w-full"
      >
        <Pencil className="w-3 h-3" />
        Edit
      </button>
    </div>
  );
}

function CombinedOutputEditor({
  rows,
  onChange,
}: {
  rows: OutputRow[];
  onChange: (rows: OutputRow[]) => void;
}) {
  const updateRow = (index: number, patch: Partial<OutputRow>) => {
    onChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  };

  const removeRow = (index: number) => {
    onChange(rows.filter((_, i) => i !== index));
  };

  const addRow = () => {
    onChange([
      ...rows,
      {
        key: "",
        type: "string",
        required: false,
        secret: false,
        description: "",
        expression: "",
      },
    ]);
  };

  return (
    <div className="space-y-3">
      <div className="text-xs text-gray-500">
        Each entry defines one field of the workflow's final{" "}
        <code className="px-1 bg-gray-100 rounded">result</code> JSON: a name &amp;
        type (the schema) plus a template expression evaluated on completion.
        Expressions reference data via{" "}
        <code className="px-1 bg-gray-100 rounded">{"{{ parameters.x }}"}</code>,{" "}
        <code className="px-1 bg-gray-100 rounded">{"{{ workflow.var }}"}</code>, or{" "}
        <code className="px-1 bg-gray-100 rounded">{"{{ task.NAME.result }}"}</code>.
        Pure <code className="px-1 bg-gray-100 rounded">{"{{ … }}"}</code>{" "}
        expressions preserve the underlying JSON type.
      </div>
      {rows.length === 0 ? (
        <div className="text-xs text-gray-400 italic px-3 py-6 text-center border-2 border-dashed border-gray-200 rounded-lg">
          No outputs yet.
        </div>
      ) : (
        <div className="space-y-3">
          {rows.map((row, index) => (
            <div
              key={index}
              className="border border-gray-200 rounded-lg p-3 bg-gray-50/50"
            >
              <div className="flex items-start gap-2">
                <div className="flex-1 space-y-2">
                  <div className="grid grid-cols-[1fr_auto_auto_auto] gap-2 items-end">
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 uppercase tracking-wider mb-1">
                        Name
                      </label>
                      <input
                        type="text"
                        value={row.key}
                        onChange={(e) =>
                          updateRow(index, { key: e.target.value })
                        }
                        placeholder="e.g. headline"
                        className="w-full px-2 py-1.5 text-sm font-mono border border-gray-200 rounded focus:outline-none focus:ring-2 focus:ring-violet-500 focus:border-transparent"
                      />
                    </div>
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 uppercase tracking-wider mb-1">
                        Type
                      </label>
                      <select
                        value={row.type}
                        onChange={(e) =>
                          updateRow(index, { type: e.target.value })
                        }
                        className="px-2 py-1.5 text-sm border border-gray-200 rounded bg-white focus:outline-none focus:ring-2 focus:ring-violet-500 focus:border-transparent"
                      >
                        {OUTPUT_TYPES.map((t) => (
                          <option key={t} value={t}>
                            {t}
                          </option>
                        ))}
                      </select>
                    </div>
                    <label className="flex items-center gap-1 text-[11px] text-gray-600 pb-1.5 cursor-pointer select-none">
                      <input
                        type="checkbox"
                        checked={row.required}
                        onChange={(e) =>
                          updateRow(index, { required: e.target.checked })
                        }
                        className="rounded border-gray-300"
                      />
                      required
                    </label>
                    <label className="flex items-center gap-1 text-[11px] text-gray-600 pb-1.5 cursor-pointer select-none">
                      <input
                        type="checkbox"
                        checked={row.secret}
                        onChange={(e) =>
                          updateRow(index, { secret: e.target.checked })
                        }
                        className="rounded border-gray-300"
                      />
                      secret
                    </label>
                  </div>
                  <div>
                    <label className="block text-[10px] font-medium text-gray-500 uppercase tracking-wider mb-1">
                      Description
                    </label>
                    <input
                      type="text"
                      value={row.description}
                      onChange={(e) =>
                        updateRow(index, { description: e.target.value })
                      }
                      placeholder="Optional description"
                      className="w-full px-2 py-1.5 text-xs border border-gray-200 rounded focus:outline-none focus:ring-2 focus:ring-violet-500 focus:border-transparent"
                    />
                  </div>
                  <div>
                    <label className="block text-[10px] font-medium text-gray-500 uppercase tracking-wider mb-1">
                      Expression
                    </label>
                    <textarea
                      value={row.expression}
                      onChange={(e) =>
                        updateRow(index, { expression: e.target.value })
                      }
                      placeholder="{{ task.fetch.result.data.headline }}"
                      rows={3}
                      className="w-full px-2 py-1.5 text-xs font-mono border border-gray-200 rounded focus:outline-none focus:ring-2 focus:ring-violet-500 focus:border-transparent resize-y"
                    />
                  </div>
                </div>
                <button
                  onClick={() => removeRow(index)}
                  className="p-1.5 rounded text-gray-400 hover:text-red-500 hover:bg-red-50 transition-colors"
                  title="Remove output"
                >
                  <Trash2 className="w-4 h-4" />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
      <button
        onClick={addRow}
        className="flex items-center gap-1.5 px-3 py-2 text-xs font-medium text-violet-600 hover:text-violet-700 hover:bg-violet-50 rounded-lg border border-dashed border-violet-300 transition-colors w-full justify-center"
      >
        <Plus className="w-3.5 h-3.5" />
        Add output
      </button>
    </div>
  );
}

function CombinedOutputSummaryList({
  output,
  outputMap,
  emptyMessage,
  onEdit,
}: {
  output: Record<string, ParamDefinition>;
  outputMap: Record<string, string>;
  emptyMessage: string;
  onEdit: () => void;
}) {
  const keys = Array.from(
    new Set([...Object.keys(output), ...Object.keys(outputMap)]),
  );

  if (keys.length === 0) {
    return (
      <button
        onClick={onEdit}
        className="w-full px-3 py-4 border-2 border-dashed border-gray-200 rounded-lg text-center hover:border-violet-300 hover:bg-violet-50/30 transition-colors group"
      >
        <Plus className="w-4 h-4 text-gray-300 group-hover:text-violet-400 mx-auto mb-1" />
        <span className="text-[11px] text-gray-400 group-hover:text-violet-500">
          {emptyMessage}
        </span>
      </button>
    );
  }

  return (
    <div className="space-y-1">
      {keys.map((key) => {
        const def = output[key];
        const expr = outputMap[key];
        return (
          <div
            key={key}
            className="px-2 py-1.5 bg-white border border-gray-150 rounded-md"
          >
            <div className="flex items-center gap-1.5">
              <span className="font-mono text-[11px] font-medium text-gray-800 truncate">
                {key}
              </span>
              {def?.required && (
                <span className="text-[9px] font-semibold text-red-500">*</span>
              )}
              {def?.type && (
                <span className="text-[10px] text-violet-600/70">
                  {def.type}
                </span>
              )}
              {def?.secret && (
                <span className="text-[9px] text-amber-500">secret</span>
              )}
            </div>
            <div
              className="font-mono text-[10px] text-violet-600/80 truncate"
              title={expr}
            >
              {expr ? (
                expr
              ) : (
                <span className="italic text-gray-300">(no expression)</span>
              )}
            </div>
          </div>
        );
      })}
      <button
        onClick={onEdit}
        className="flex items-center gap-1 px-2 py-1 text-[11px] text-gray-400 hover:text-violet-600 transition-colors w-full"
      >
        <Pencil className="w-3 h-3" />
        Edit
      </button>
    </div>
  );
}

export default function WorkflowInputsPanel({
  label,
  version,
  description,
  tags,
  referenceVisibility,
  referenceAllowedPackRefs,
  cancellationPolicy,
  parameters,
  output,
  outputMap,
  onLabelChange,
  onVersionChange,
  onDescriptionChange,
  onTagsChange,
  onReferenceVisibilityChange,
  onReferenceAllowedPackRefsChange,
  onCancellationPolicyChange,
  onParametersChange,
  onOutputChange,
  onOutputMapChange,
}: WorkflowInputsPanelProps) {
  const [modalTarget, setModalTarget] = useState<ModalTarget>(null);

  // Draft state for the schema modal so changes only apply on confirm
  const [draftSchema, setDraftSchema] = useState<
    Record<string, ParamDefinition>
  >({});

  // Draft state for the combined Output editor (schema + expression).
  const [draftOutputRows, setDraftOutputRows] = useState<OutputRow[]>([]);

  const openModal = (target: ModalTarget) => {
    if (target === "parameters") {
      setDraftSchema({ ...parameters });
    } else if (target === "output") {
      const rows = buildOutputRows(output, outputMap);
      setDraftOutputRows(
        rows.length > 0
          ? rows
          : [
              {
                key: "",
                type: "string",
                required: false,
                secret: false,
                description: "",
                expression: "",
              },
            ],
      );
    }
    setModalTarget(target);
  };

  const handleConfirm = () => {
    if (modalTarget === "parameters") {
      onParametersChange(draftSchema);
    } else if (modalTarget === "output") {
      const { schema, outputMap: nextMap } = rowsToSchemaAndMap(draftOutputRows);
      onOutputChange(schema);
      onOutputMapChange(nextMap);
    }
    setModalTarget(null);
  };

  const handleCancel = () => {
    setModalTarget(null);
  };

  const modalLabel =
    modalTarget === "parameters" ? "Input Parameters" : "Output";

  return (
    <>
      <div className="flex flex-col h-full overflow-hidden">
        <div className="flex-1 overflow-y-auto p-3 space-y-4">
          <div>
            <div className="flex items-center gap-1.5 mb-2">
              <SlidersHorizontal className="w-3.5 h-3.5 text-blue-500" />
              <h4 className="text-xs font-semibold text-gray-600 uppercase tracking-wider">
                Workflow
              </h4>
            </div>
            <div className="space-y-2.5 rounded-lg border border-gray-200 bg-white p-3">
              <div>
                <label className="block text-[11px] font-medium text-gray-600 mb-1">
                  Label
                </label>
                <input
                  type="text"
                  value={label}
                  onChange={(e) => onLabelChange(e.target.value)}
                  className="w-full px-2.5 py-2 border border-gray-300 rounded-md text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                  placeholder="Workflow Label"
                />
              </div>

              <div>
                <label className="block text-[11px] font-medium text-gray-600 mb-1">
                  Description
                </label>
                <input
                  type="text"
                  value={description}
                  onChange={(e) => onDescriptionChange(e.target.value)}
                  className="w-full px-2.5 py-2 border border-gray-300 rounded-md text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                  placeholder="Workflow description"
                />
              </div>

              <div className="grid grid-cols-1 gap-2">
                <div>
                  <label className="block text-[11px] font-medium text-gray-600 mb-1">
                    Version
                  </label>
                  <input
                    type="text"
                    value={version}
                    onChange={(e) => onVersionChange(e.target.value)}
                    className="w-full px-2.5 py-2 border border-gray-300 rounded-md text-sm font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                    placeholder="1.0.0"
                  />
                </div>

                <div>
                  <label className="block text-[11px] font-medium text-gray-600 mb-1">
                    Tags
                  </label>
                  <input
                    type="text"
                    value={tags.join(", ")}
                    onChange={(e) =>
                      onTagsChange(
                        e.target.value
                          .split(",")
                          .map((tag) => tag.trim())
                          .filter(Boolean),
                      )
                    }
                    className="w-full px-2.5 py-2 border border-gray-300 rounded-md text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                    placeholder="tag-one, tag-two"
                  />
                </div>
              </div>

              <div>
                <label className="block text-[11px] font-medium text-gray-600 mb-1">
                  Cancellation Policy
                </label>
                <select
                  value={cancellationPolicy}
                  onChange={(e) =>
                    onCancellationPolicyChange(
                      e.target.value as CancellationPolicy,
                    )
                  }
                  className="w-full px-2.5 py-2 border border-gray-300 rounded-md text-sm text-gray-700 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 bg-white"
                  title="Controls how running tasks behave when the workflow is cancelled"
                >
                  {Object.entries(CANCELLATION_POLICY_LABELS).map(
                    ([value, optionLabel]) => (
                      <option key={value} value={value}>
                        {optionLabel}
                      </option>
                    ),
                  )}
                </select>
              </div>

              <div>
                <label className="block text-[11px] font-medium text-gray-600 mb-1">
                  Reference Visibility
                </label>
                <select
                  value={referenceVisibility}
                  onChange={(e) =>
                    onReferenceVisibilityChange(
                      e.target.value as ReferenceVisibility,
                    )
                  }
                  className="w-full px-2.5 py-2 border border-gray-300 rounded-md text-sm text-gray-700 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 bg-white"
                >
                  <option value="public">Public - any pack may reference</option>
                  <option value="private">
                    Private - only this pack may reference
                  </option>
                  <option value="restricted">
                    Restricted - this pack plus allowed packs
                  </option>
                </select>
                <p className="mt-1 text-[10px] text-gray-400">
                  Controls which packs may call this workflow action from rules,
                  workflows, and queues.
                </p>
              </div>

              {referenceVisibility === "restricted" && (
                <div>
                  <label className="block text-[11px] font-medium text-gray-600 mb-1">
                    Allowed Pack Refs
                  </label>
                  <textarea
                    value={referenceAllowedPackRefs.join("\n")}
                    onChange={(e) =>
                      onReferenceAllowedPackRefsChange(
                        e.target.value
                          .split(/\r?\n|,/)
                          .map((packRef) => packRef.trim())
                          .filter(Boolean),
                      )
                    }
                    rows={3}
                    placeholder={"incident_response\ndeployments"}
                    className="w-full px-2.5 py-2 border border-gray-300 rounded-md text-sm font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                  />
                  <p className="mt-1 text-[10px] text-gray-400">
                    Enter one pack ref per line or separate refs with commas. The
                    workflow action's own pack is always allowed.
                  </p>
                </div>
              )}
            </div>
          </div>

          {/* Input Parameters */}
          <div className="border-t border-gray-200 pt-3">
            <div className="flex items-center justify-between mb-1.5">
              <div className="flex items-center gap-1.5">
                <LogIn className="w-3.5 h-3.5 text-green-500" />
                <h4 className="text-xs font-semibold text-gray-600 uppercase tracking-wider">
                  Inputs
                </h4>
              </div>
              {Object.keys(parameters).length > 0 && (
                <span className="text-[10px] text-gray-400">
                  {Object.keys(parameters).length}
                </span>
              )}
            </div>
            <p className="text-[10px] text-gray-400 mb-2">
              Referenced via{" "}
              <code className="px-0.5 bg-gray-100 rounded">
                {"{{ params.<name> }}"}
              </code>
            </p>
            <ParamSummaryList
              schema={parameters}
              emptyMessage="Add input parameters"
              onEdit={() => openModal("parameters")}
            />
          </div>

          {/* Output (combined schema + expression) */}
          <div className="border-t border-gray-200 pt-3">
            <div className="flex items-center justify-between mb-1.5">
              <div className="flex items-center gap-1.5">
                <LogOut className="w-3.5 h-3.5 text-violet-500" />
                <h4 className="text-xs font-semibold text-gray-600 uppercase tracking-wider">
                  Output
                </h4>
              </div>
              {Object.keys(output).length + Object.keys(outputMap).length >
                0 && (
                <span className="text-[10px] text-gray-400">
                  {
                    new Set([
                      ...Object.keys(output),
                      ...Object.keys(outputMap),
                    ]).size
                  }
                </span>
              )}
            </div>
            <p className="text-[10px] text-gray-400 mb-2">
              Fields the workflow produces on completion. Each has a type
              (schema) and a template expression evaluated at completion.
            </p>
            <CombinedOutputSummaryList
              output={output}
              outputMap={outputMap}
              emptyMessage="Add output fields"
              onEdit={() => openModal("output")}
            />
          </div>
        </div>
      </div>

      {/* Full-screen modal for SchemaBuilder editing */}
      {modalTarget && (
        <div className="fixed inset-0 z-[70] flex items-center justify-center">
          {/* Backdrop */}
          <div
            className="absolute inset-0 bg-black/40"
            onClick={handleCancel}
          />
          {/* Modal */}
          <div className="relative bg-white rounded-xl shadow-2xl border border-gray-200 flex flex-col w-full max-w-2xl max-h-[80vh] mx-4">
            {/* Header */}
            <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200 flex-shrink-0">
              <div>
                <h2 className="text-base font-semibold text-gray-900">
                  {modalLabel}
                </h2>
                <p className="text-xs text-gray-500 mt-0.5">
                  {modalTarget === "parameters"
                    ? "Define the inputs this workflow accepts when executed."
                    : "Define the fields this workflow produces on completion. Each entry has a name & type (the schema) plus a template expression."}
                </p>
              </div>
              <button
                onClick={handleCancel}
                className="p-1.5 rounded-lg hover:bg-gray-100 text-gray-400 hover:text-gray-600 transition-colors"
              >
                <X className="w-5 h-5" />
              </button>
            </div>

            {/* Body */}
            <div className="flex-1 overflow-y-auto px-6 py-4">
              {modalTarget === "output" ? (
                <CombinedOutputEditor
                  rows={draftOutputRows}
                  onChange={setDraftOutputRows}
                />
              ) : (
                <SchemaBuilder
                  value={draftSchema}
                  onChange={(schema) =>
                    setDraftSchema(
                      schema as unknown as Record<string, ParamDefinition>,
                    )
                  }
                  placeholder='{"message": {"type": "string", "required": true}}'
                />
              )}
            </div>

            {/* Footer */}
            <div className="flex items-center justify-end gap-2 px-6 py-3 border-t border-gray-200 bg-gray-50 rounded-b-xl flex-shrink-0">
              <button
                onClick={handleCancel}
                className="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors"
              >
                Cancel
              </button>
              <button
                onClick={handleConfirm}
                className="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700 transition-colors shadow-sm"
              >
                Apply
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
