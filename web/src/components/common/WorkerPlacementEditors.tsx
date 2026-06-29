import { useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import type { WorkerToleration } from "@/api/models/WorkerToleration";
import { TaintEffect } from "@/api/models/TaintEffect";
import { TolerationOperator } from "@/api/models/TolerationOperator";
import type { WorkerAffinity } from "@/api/models/WorkerAffinity";
import type { WorkerSelectorTerm } from "@/api/models/WorkerSelectorTerm";
import type { PreferredWorkerSelectorTerm } from "@/api/models/PreferredWorkerSelectorTerm";

// ── Worker Selector (key-value pairs) ────────────────────────────────

interface SelectorEntry {
  key: string;
  value: string;
}

export function WorkerSelectorEditor({
  value,
  onChange,
}: {
  value: Record<string, string>;
  onChange: (v: Record<string, string>) => void;
}) {
  const entries: SelectorEntry[] = Object.entries(value).map(([key, val]) => ({
    key,
    value: val,
  }));

  const updateEntry = (index: number, field: "key" | "value", val: string) => {
    const updated = [...entries];
    updated[index] = { ...updated[index], [field]: val };
    onChange(
      Object.fromEntries(
        updated.filter((e) => e.key).map((e) => [e.key, e.value]),
      ),
    );
  };

  const addEntry = () => {
    onChange({ ...value, "": "" });
  };

  const removeEntry = (index: number) => {
    const updated = entries.filter((_, i) => i !== index);
    onChange(Object.fromEntries(updated.map((e) => [e.key, e.value])));
  };

  return (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1">
        Worker Selector
      </label>
      <p className="text-xs text-gray-500 mb-2">
        Exact label requirements — all must match for a worker to be eligible.
      </p>
      {entries.length === 0 ? (
        <p className="text-xs text-gray-400 italic mb-2">
          No selector labels configured.
        </p>
      ) : (
        <div className="space-y-2 mb-2">
          {entries.map((entry, i) => (
            <div key={i} className="flex items-center gap-2">
              <input
                type="text"
                value={entry.key}
                onChange={(e) => updateEntry(i, "key", e.target.value)}
                placeholder="label key"
                className="flex-1 px-2 py-1.5 border border-gray-300 rounded text-sm font-mono focus:ring-blue-500 focus:border-blue-500"
              />
              <span className="text-gray-400">=</span>
              <input
                type="text"
                value={entry.value}
                onChange={(e) => updateEntry(i, "value", e.target.value)}
                placeholder="value"
                className="flex-1 px-2 py-1.5 border border-gray-300 rounded text-sm font-mono focus:ring-blue-500 focus:border-blue-500"
              />
              <button
                type="button"
                onClick={() => removeEntry(i)}
                className="p-1 text-gray-400 hover:text-red-500"
                title="Remove"
              >
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
          ))}
        </div>
      )}
      <button
        type="button"
        onClick={addEntry}
        className="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-800"
      >
        <Plus className="h-3 w-3" /> Add label
      </button>
    </div>
  );
}

// ── Worker Tolerations ───────────────────────────────────────────────

export function WorkerTolerationsEditor({
  value,
  onChange,
}: {
  value: WorkerToleration[];
  onChange: (v: WorkerToleration[]) => void;
}) {
  const updateToleration = (
    index: number,
    patch: Partial<WorkerToleration>,
  ) => {
    const updated = [...value];
    updated[index] = { ...updated[index], ...patch };
    // Clear value when operator is Exists
    if (patch.operator === TolerationOperator.EXISTS) {
      updated[index].value = null;
    }
    onChange(updated);
  };

  const addToleration = () => {
    onChange([
      ...value,
      { key: "", operator: TolerationOperator.EQUAL, value: "", effect: null },
    ]);
  };

  const removeToleration = (index: number) => {
    onChange(value.filter((_, i) => i !== index));
  };

  return (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1">
        Worker Tolerations
      </label>
      <p className="text-xs text-gray-500 mb-2">
        Allow scheduling onto workers with matching taints.
      </p>
      {value.length === 0 ? (
        <p className="text-xs text-gray-400 italic mb-2">
          No tolerations configured.
        </p>
      ) : (
        <div className="space-y-3 mb-2">
          {value.map((t, i) => (
            <div
              key={i}
              className="border border-gray-200 rounded-md p-3 bg-gray-50"
            >
              <div className="flex items-center justify-between mb-2">
                <span className="text-xs font-medium text-gray-500">
                  Toleration {i + 1}
                </span>
                <button
                  type="button"
                  onClick={() => removeToleration(i)}
                  className="p-0.5 text-gray-400 hover:text-red-500"
                  title="Remove toleration"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-xs text-gray-500 mb-0.5">
                    Key
                  </label>
                  <input
                    type="text"
                    value={t.key}
                    onChange={(e) =>
                      updateToleration(i, { key: e.target.value })
                    }
                    placeholder="e.g. gpu"
                    className="w-full px-2 py-1.5 border border-gray-300 rounded text-sm font-mono focus:ring-blue-500 focus:border-blue-500"
                  />
                </div>
                <div>
                  <label className="block text-xs text-gray-500 mb-0.5">
                    Operator
                  </label>
                  <select
                    value={t.operator ?? TolerationOperator.EQUAL}
                    onChange={(e) =>
                      updateToleration(i, {
                        operator: e.target.value as TolerationOperator,
                      })
                    }
                    className="w-full px-2 py-1.5 border border-gray-300 rounded text-sm focus:ring-blue-500 focus:border-blue-500"
                  >
                    <option value={TolerationOperator.EQUAL}>Equal</option>
                    <option value={TolerationOperator.EXISTS}>Exists</option>
                  </select>
                </div>
                {(t.operator ?? TolerationOperator.EQUAL) ===
                  TolerationOperator.EQUAL && (
                  <div>
                    <label className="block text-xs text-gray-500 mb-0.5">
                      Value
                    </label>
                    <input
                      type="text"
                      value={t.value ?? ""}
                      onChange={(e) =>
                        updateToleration(i, { value: e.target.value })
                      }
                      placeholder="e.g. nvidia"
                      className="w-full px-2 py-1.5 border border-gray-300 rounded text-sm font-mono focus:ring-blue-500 focus:border-blue-500"
                    />
                  </div>
                )}
                <div>
                  <label className="block text-xs text-gray-500 mb-0.5">
                    Effect
                  </label>
                  <select
                    value={t.effect ?? ""}
                    onChange={(e) =>
                      updateToleration(i, {
                        effect: (e.target.value || null) as TaintEffect | null,
                      })
                    }
                    className="w-full px-2 py-1.5 border border-gray-300 rounded text-sm focus:ring-blue-500 focus:border-blue-500"
                  >
                    <option value="">Any effect</option>
                    <option value={TaintEffect.NO_SCHEDULE}>NoSchedule</option>
                    <option value={TaintEffect.PREFER_NO_SCHEDULE}>
                      PreferNoSchedule
                    </option>
                  </select>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
      <button
        type="button"
        onClick={addToleration}
        className="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-800"
      >
        <Plus className="h-3 w-3" /> Add toleration
      </button>
    </div>
  );
}

// ── Worker Affinity ──────────────────────────────────────────────────

function MatchLabelsEditor({
  labels,
  onChange,
}: {
  labels: Record<string, string>;
  onChange: (v: Record<string, string>) => void;
}) {
  const entries = Object.entries(labels);

  const updateEntry = (index: number, field: "key" | "value", val: string) => {
    const updated = [...entries];
    updated[index] =
      field === "key" ? [val, updated[index][1]] : [updated[index][0], val];
    onChange(Object.fromEntries(updated.filter(([k]) => k)));
  };

  const addEntry = () => onChange({ ...labels, "": "" });
  const removeEntry = (index: number) => {
    onChange(Object.fromEntries(entries.filter((_, i) => i !== index)));
  };

  return (
    <div className="space-y-1.5">
      {entries.map(([key, val], i) => (
        <div key={i} className="flex items-center gap-1.5">
          <input
            type="text"
            value={key}
            onChange={(e) => updateEntry(i, "key", e.target.value)}
            placeholder="key"
            className="flex-1 px-2 py-1 border border-gray-300 rounded text-xs font-mono focus:ring-blue-500 focus:border-blue-500"
          />
          <span className="text-gray-400 text-xs">=</span>
          <input
            type="text"
            value={val}
            onChange={(e) => updateEntry(i, "value", e.target.value)}
            placeholder="value"
            className="flex-1 px-2 py-1 border border-gray-300 rounded text-xs font-mono focus:ring-blue-500 focus:border-blue-500"
          />
          <button
            type="button"
            onClick={() => removeEntry(i)}
            className="p-0.5 text-gray-400 hover:text-red-500"
          >
            <Trash2 className="h-3 w-3" />
          </button>
        </div>
      ))}
      <button
        type="button"
        onClick={addEntry}
        className="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-800"
      >
        <Plus className="h-3 w-3" /> Add label
      </button>
    </div>
  );
}

function SelectorTermEditor({
  term,
  onChange,
  onRemove,
}: {
  term: WorkerSelectorTerm;
  onChange: (t: WorkerSelectorTerm) => void;
  onRemove: () => void;
}) {
  return (
    <div className="border border-gray-200 rounded p-2.5 bg-white">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs font-medium text-gray-500">Match Labels</span>
        <button
          type="button"
          onClick={onRemove}
          className="p-0.5 text-gray-400 hover:text-red-500"
          title="Remove term"
        >
          <Trash2 className="h-3 w-3" />
        </button>
      </div>
      <MatchLabelsEditor
        labels={term.match_labels ?? {}}
        onChange={(labels) => onChange({ ...term, match_labels: labels })}
      />
    </div>
  );
}

function AffinityTermList({
  label,
  description,
  terms,
  onChange,
}: {
  label: string;
  description: string;
  terms: WorkerSelectorTerm[];
  onChange: (terms: WorkerSelectorTerm[]) => void;
}) {
  return (
    <div>
      <label className="block text-xs font-medium text-gray-600 mb-0.5">
        {label}
      </label>
      <p className="text-xs text-gray-400 mb-2">{description}</p>
      {terms.length > 0 && (
        <div className="space-y-2 mb-2">
          {terms.map((term, i) => (
            <SelectorTermEditor
              key={i}
              term={term}
              onChange={(t) => {
                const updated = [...terms];
                updated[i] = t;
                onChange(updated);
              }}
              onRemove={() => onChange(terms.filter((_, j) => j !== i))}
            />
          ))}
        </div>
      )}
      <button
        type="button"
        onClick={() => onChange([...terms, { match_labels: {} }])}
        className="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-800"
      >
        <Plus className="h-3 w-3" /> Add term
      </button>
    </div>
  );
}

function PreferredAffinityTermList({
  terms,
  onChange,
}: {
  terms: PreferredWorkerSelectorTerm[];
  onChange: (terms: PreferredWorkerSelectorTerm[]) => void;
}) {
  return (
    <div>
      <label className="block text-xs font-medium text-gray-600 mb-0.5">
        Preferred
      </label>
      <p className="text-xs text-gray-400 mb-2">
        Workers matching these labels are scored higher. Weight determines
        priority.
      </p>
      {terms.length > 0 && (
        <div className="space-y-2 mb-2">
          {terms.map((pt, i) => (
            <div
              key={i}
              className="border border-gray-200 rounded p-2.5 bg-white"
            >
              <div className="flex items-center justify-between mb-2">
                <div className="flex items-center gap-2">
                  <span className="text-xs font-medium text-gray-500">
                    Weight
                  </span>
                  <input
                    type="number"
                    min={1}
                    max={100}
                    value={pt.weight ?? 1}
                    onChange={(e) => {
                      const updated = [...terms];
                      updated[i] = {
                        ...pt,
                        weight: parseInt(e.target.value) || 1,
                      };
                      onChange(updated);
                    }}
                    className="w-16 px-2 py-0.5 border border-gray-300 rounded text-xs font-mono focus:ring-blue-500 focus:border-blue-500"
                  />
                </div>
                <button
                  type="button"
                  onClick={() => onChange(terms.filter((_, j) => j !== i))}
                  className="p-0.5 text-gray-400 hover:text-red-500"
                  title="Remove preferred term"
                >
                  <Trash2 className="h-3 w-3" />
                </button>
              </div>
              <span className="text-xs font-medium text-gray-500 block mb-1">
                Match Labels
              </span>
              <MatchLabelsEditor
                labels={pt.preference?.match_labels ?? {}}
                onChange={(labels) => {
                  const updated = [...terms];
                  updated[i] = {
                    ...pt,
                    preference: {
                      ...pt.preference,
                      match_labels: labels,
                    },
                  };
                  onChange(updated);
                }}
              />
            </div>
          ))}
        </div>
      )}
      <button
        type="button"
        onClick={() =>
          onChange([...terms, { preference: { match_labels: {} }, weight: 1 }])
        }
        className="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-800"
      >
        <Plus className="h-3 w-3" /> Add preferred term
      </button>
    </div>
  );
}

export function WorkerAffinityEditor({
  value,
  onChange,
}: {
  value: WorkerAffinity;
  onChange: (v: WorkerAffinity) => void;
}) {
  const [expandedSections, setExpandedSections] = useState<Set<string>>(() => {
    const initial = new Set<string>();
    if ((value.required?.length ?? 0) > 0) initial.add("required");
    if ((value.preferred?.length ?? 0) > 0) initial.add("preferred");
    if ((value.anti_affinity?.length ?? 0) > 0) initial.add("anti");
    return initial;
  });

  const toggle = (section: string) => {
    setExpandedSections((prev) => {
      const next = new Set(prev);
      if (next.has(section)) next.delete(section);
      else next.add(section);
      return next;
    });
  };

  return (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1">
        Worker Affinity
      </label>
      <p className="text-xs text-gray-500 mb-3">
        Fine-grained worker selection by label matching.
      </p>

      <div className="space-y-3">
        {/* Required */}
        <div className="border border-gray-200 rounded-md overflow-hidden">
          <button
            type="button"
            onClick={() => toggle("required")}
            className="w-full flex items-center justify-between px-3 py-2 bg-gray-50 hover:bg-gray-100 text-sm font-medium text-gray-700"
          >
            <span>
              Required{" "}
              {(value.required?.length ?? 0) > 0 && (
                <span className="ml-1 text-xs text-gray-500">
                  ({value.required!.length} term
                  {value.required!.length !== 1 ? "s" : ""})
                </span>
              )}
            </span>
            <span className="text-gray-400 text-xs">
              {expandedSections.has("required") ? "▾" : "▸"}
            </span>
          </button>
          {expandedSections.has("required") && (
            <div className="p-3 border-t border-gray-200 bg-gray-50/50">
              <AffinityTermList
                label=""
                description="Worker must match ALL labels in at least one term."
                terms={value.required ?? []}
                onChange={(terms) => onChange({ ...value, required: terms })}
              />
            </div>
          )}
        </div>

        {/* Preferred */}
        <div className="border border-gray-200 rounded-md overflow-hidden">
          <button
            type="button"
            onClick={() => toggle("preferred")}
            className="w-full flex items-center justify-between px-3 py-2 bg-gray-50 hover:bg-gray-100 text-sm font-medium text-gray-700"
          >
            <span>
              Preferred{" "}
              {(value.preferred?.length ?? 0) > 0 && (
                <span className="ml-1 text-xs text-gray-500">
                  ({value.preferred!.length} term
                  {value.preferred!.length !== 1 ? "s" : ""})
                </span>
              )}
            </span>
            <span className="text-gray-400 text-xs">
              {expandedSections.has("preferred") ? "▾" : "▸"}
            </span>
          </button>
          {expandedSections.has("preferred") && (
            <div className="p-3 border-t border-gray-200 bg-gray-50/50">
              <PreferredAffinityTermList
                terms={value.preferred ?? []}
                onChange={(terms) => onChange({ ...value, preferred: terms })}
              />
            </div>
          )}
        </div>

        {/* Anti-Affinity */}
        <div className="border border-gray-200 rounded-md overflow-hidden">
          <button
            type="button"
            onClick={() => toggle("anti")}
            className="w-full flex items-center justify-between px-3 py-2 bg-gray-50 hover:bg-gray-100 text-sm font-medium text-gray-700"
          >
            <span>
              Anti-Affinity{" "}
              {(value.anti_affinity?.length ?? 0) > 0 && (
                <span className="ml-1 text-xs text-gray-500">
                  ({value.anti_affinity!.length} term
                  {value.anti_affinity!.length !== 1 ? "s" : ""})
                </span>
              )}
            </span>
            <span className="text-gray-400 text-xs">
              {expandedSections.has("anti") ? "▾" : "▸"}
            </span>
          </button>
          {expandedSections.has("anti") && (
            <div className="p-3 border-t border-gray-200 bg-gray-50/50">
              <AffinityTermList
                label=""
                description="Workers matching any term are excluded."
                terms={value.anti_affinity ?? []}
                onChange={(terms) =>
                  onChange({ ...value, anti_affinity: terms })
                }
              />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
