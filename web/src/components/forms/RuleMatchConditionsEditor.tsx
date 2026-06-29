import { useEffect, useId, useState } from "react";
import { Braces, ListFilter, Plus, Trash2 } from "lucide-react";
import SearchableSelect from "@/components/common/SearchableSelect";

type JsonValue =
  string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

type ConditionOperator = "equals" | "not_equals" | "contains";
type ConditionValueType = "string" | "number" | "boolean" | "null" | "json";
type EditorMode = "guided" | "raw";

interface ConditionRow {
  id: string;
  field: string;
  operator: ConditionOperator;
  valueType: ConditionValueType;
  valueInput: string;
}

interface RuleMatchConditionsEditorProps {
  value: unknown;
  onChange: (value: JsonValue[] | JsonValue | undefined) => void;
  error?: string;
  onErrorChange?: (message?: string) => void;
}

const OPERATOR_OPTIONS = [
  {
    value: "equals",
    label: "Equals",
  },
  {
    value: "not_equals",
    label: "Does not equal",
  },
  {
    value: "contains",
    label: "Contains",
  },
] satisfies Array<{ value: ConditionOperator; label: string }>;

const VALUE_TYPE_OPTIONS = [
  {
    value: "string",
    label: "Text",
  },
  {
    value: "number",
    label: "Number",
  },
  {
    value: "boolean",
    label: "True/False",
  },
  {
    value: "null",
    label: "Empty",
  },
  {
    value: "json",
    label: "JSON",
  },
] satisfies Array<{ value: ConditionValueType; label: string }>;

const DEFAULT_OPERATOR: ConditionOperator = "equals";
const DEFAULT_VALUE_TYPE: ConditionValueType = "string";

function createRow(partial?: Partial<ConditionRow>): ConditionRow {
  return {
    id: Math.random().toString(36).slice(2, 10),
    field: partial?.field || "",
    operator: partial?.operator || DEFAULT_OPERATOR,
    valueType: partial?.valueType || DEFAULT_VALUE_TYPE,
    valueInput: partial?.valueInput || "",
  };
}

function inferValueType(value: unknown): ConditionValueType {
  if (value === null) {
    return "null";
  }
  if (typeof value === "string") {
    return "string";
  }
  if (typeof value === "number") {
    return "number";
  }
  if (typeof value === "boolean") {
    return "boolean";
  }
  return "json";
}

function formatValueInput(
  value: unknown,
  valueType: ConditionValueType,
): string {
  if (valueType === "null") {
    return "";
  }
  if (valueType === "json") {
    return JSON.stringify(value, null, 2);
  }
  return String(value ?? "");
}

function isGuidedCondition(value: unknown): value is {
  field: string;
  operator: ConditionOperator;
  value: unknown;
} {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }

  const condition = value as Record<string, unknown>;
  return (
    typeof condition.field === "string" &&
    typeof condition.operator === "string" &&
    Object.prototype.hasOwnProperty.call(condition, "value") &&
    OPERATOR_OPTIONS.some((option) => option.value === condition.operator)
  );
}

function parseInitialState(value: unknown): {
  mode: EditorMode;
  rows: ConditionRow[];
  rawText: string;
  unsupportedMessage?: string;
} {
  if (
    value == null ||
    (Array.isArray(value) && value.length === 0) ||
    (typeof value === "object" &&
      !Array.isArray(value) &&
      Object.keys(value as Record<string, unknown>).length === 0)
  ) {
    return {
      mode: "guided",
      rows: [],
      rawText: "",
    };
  }

  if (Array.isArray(value) && value.every(isGuidedCondition)) {
    return {
      mode: "guided",
      rows: value.map((condition) => {
        const valueType = inferValueType(condition.value);
        return createRow({
          field: condition.field,
          operator: condition.operator,
          valueType,
          valueInput: formatValueInput(condition.value, valueType),
        });
      }),
      rawText: JSON.stringify(value, null, 2),
    };
  }

  return {
    mode: "raw",
    rows: [],
    rawText: JSON.stringify(value, null, 2),
    unsupportedMessage:
      "This rule uses a condition shape outside the guided builder. Edit it in raw JSON to preserve it.",
  };
}

function parseConditionValue(row: ConditionRow): {
  value?: JsonValue;
  error?: string;
} {
  switch (row.valueType) {
    case "string":
      return { value: row.valueInput };
    case "number": {
      const trimmed = row.valueInput.trim();
      if (!trimmed) {
        return { error: "Number value is required." };
      }
      const parsed = Number(trimmed);
      if (Number.isNaN(parsed)) {
        return { error: "Enter a valid number." };
      }
      return { value: parsed };
    }
    case "boolean":
      return { value: row.valueInput === "true" };
    case "null":
      return { value: null };
    case "json":
      if (!row.valueInput.trim()) {
        return { error: "JSON value is required." };
      }
      try {
        return { value: JSON.parse(row.valueInput) as JsonValue };
      } catch {
        return { error: "Enter valid JSON." };
      }
  }
}

export default function RuleMatchConditionsEditor({
  value,
  onChange,
  error,
  onErrorChange,
}: RuleMatchConditionsEditorProps) {
  const fieldId = useId();
  const [mode, setMode] = useState<EditorMode>(
    () => parseInitialState(value).mode,
  );
  const [rows, setRows] = useState<ConditionRow[]>(
    () => parseInitialState(value).rows,
  );
  const [rawText, setRawText] = useState(
    () => parseInitialState(value).rawText,
  );
  const [unsupportedMessage] = useState<string | undefined>(
    () => parseInitialState(value).unsupportedMessage,
  );

  useEffect(() => {
    if (mode === "raw") {
      if (!rawText.trim()) {
        onErrorChange?.(undefined);
        onChange(undefined);
        return;
      }

      try {
        onErrorChange?.(undefined);
        onChange(JSON.parse(rawText) as JsonValue);
      } catch {
        onErrorChange?.("Invalid JSON format");
      }
      return;
    }

    const nextConditions: JsonValue[] = [];

    for (let index = 0; index < rows.length; index += 1) {
      const row = rows[index];
      if (!row.field.trim()) {
        onErrorChange?.(`Condition ${index + 1}: field is required.`);
        return;
      }

      const parsedValue = parseConditionValue(row);
      if (parsedValue.error) {
        onErrorChange?.(`Condition ${index + 1}: ${parsedValue.error}`);
        return;
      }

      nextConditions.push({
        field: row.field.trim(),
        operator: row.operator,
        value: parsedValue.value ?? null,
      });
    }

    onErrorChange?.(undefined);
    onChange(nextConditions.length > 0 ? nextConditions : undefined);
  }, [mode, onChange, onErrorChange, rawText, rows]);

  const addCondition = () => {
    setRows((current) => [...current, createRow()]);
  };

  const updateRow = (
    id: string,
    updater: (row: ConditionRow) => ConditionRow,
  ) => {
    setRows((current) =>
      current.map((row) => (row.id === id ? updater(row) : row)),
    );
  };

  const removeCondition = (id: string) => {
    setRows((current) => current.filter((row) => row.id !== id));
  };

  const currentError = error;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h4 className="text-sm font-medium text-gray-700">
            Match Conditions
          </h4>
          <p className="mt-1 text-xs text-gray-500">
            All conditions must match. Leave this empty to match every event
            from the selected trigger.
          </p>
        </div>

        <div className="inline-flex rounded-lg border border-gray-200 bg-gray-50 p-1">
          <button
            type="button"
            onClick={() => setMode("guided")}
            className={`inline-flex items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors ${
              mode === "guided"
                ? "bg-white text-gray-900 shadow-sm"
                : "text-gray-600 hover:text-gray-900"
            }`}
          >
            <ListFilter className="h-4 w-4" />
            Guided
          </button>
          <button
            type="button"
            onClick={() => setMode("raw")}
            className={`inline-flex items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors ${
              mode === "raw"
                ? "bg-white text-gray-900 shadow-sm"
                : "text-gray-600 hover:text-gray-900"
            }`}
          >
            <Braces className="h-4 w-4" />
            Raw JSON
          </button>
        </div>
      </div>

      {unsupportedMessage && (
        <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">
          {unsupportedMessage}
        </div>
      )}

      {mode === "guided" ? (
        <div className="space-y-3">
          {rows.length === 0 ? (
            <div className="rounded-xl border border-dashed border-gray-300 bg-gray-50 px-4 py-5 text-sm text-gray-500">
              No conditions configured.
            </div>
          ) : (
            rows.map((row, index) => (
              <div
                key={row.id}
                className="rounded-xl border border-gray-200 bg-gray-50/70 p-4"
              >
                <div className="mb-3 flex items-center justify-between gap-3">
                  <span className="text-sm font-medium text-gray-700">
                    Condition {index + 1}
                  </span>
                  <button
                    type="button"
                    onClick={() => removeCondition(row.id)}
                    className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-sm text-gray-500 hover:bg-white hover:text-red-600"
                  >
                    <Trash2 className="h-4 w-4" />
                    Remove
                  </button>
                </div>

                <div className="grid grid-cols-1 gap-3 xl:grid-cols-12">
                  <div className="xl:col-span-5">
                    <label
                      htmlFor={`${fieldId}-${row.id}-field`}
                      className="mb-1 block text-xs font-medium uppercase tracking-wide text-gray-500"
                    >
                      Event field
                    </label>
                    <input
                      id={`${fieldId}-${row.id}-field`}
                      type="text"
                      value={row.field}
                      onChange={(e) =>
                        updateRow(row.id, (current) => ({
                          ...current,
                          field: e.target.value,
                        }))
                      }
                      placeholder="status or nested.path"
                      className="w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
                    />
                  </div>

                  <div className="xl:col-span-3">
                    <label className="mb-1 block text-xs font-medium uppercase tracking-wide text-gray-500">
                      Operator
                    </label>
                    <SearchableSelect
                      value={row.operator}
                      onChange={(nextValue) =>
                        updateRow(row.id, (current) => ({
                          ...current,
                          operator: nextValue as ConditionOperator,
                        }))
                      }
                      options={OPERATOR_OPTIONS}
                      placeholder="Choose operator"
                    />
                  </div>

                  <div className="xl:col-span-4">
                    <label className="mb-1 block text-xs font-medium uppercase tracking-wide text-gray-500">
                      Value type
                    </label>
                    <SearchableSelect
                      value={row.valueType}
                      onChange={(nextValue) =>
                        updateRow(row.id, (current) => ({
                          ...current,
                          valueType: nextValue as ConditionValueType,
                          valueInput:
                            nextValue === "boolean"
                              ? "true"
                              : nextValue === "null"
                                ? ""
                                : current.valueInput,
                        }))
                      }
                      options={VALUE_TYPE_OPTIONS}
                      placeholder="Choose type"
                    />
                  </div>

                  <div className="xl:col-span-12">
                    <label className="mb-1 block text-xs font-medium uppercase tracking-wide text-gray-500">
                      Expected value
                    </label>

                    {row.valueType === "boolean" ? (
                      <SearchableSelect
                        value={row.valueInput || "true"}
                        onChange={(nextValue) =>
                          updateRow(row.id, (current) => ({
                            ...current,
                            valueInput: String(nextValue),
                          }))
                        }
                        options={[
                          { value: "true", label: "True" },
                          { value: "false", label: "False" },
                        ]}
                      />
                    ) : row.valueType === "null" ? (
                      <div className="rounded-lg border border-dashed border-gray-300 bg-white px-3 py-2 text-sm text-gray-500">
                        This condition matches a null value.
                      </div>
                    ) : row.valueType === "json" ? (
                      <textarea
                        value={row.valueInput}
                        onChange={(e) =>
                          updateRow(row.id, (current) => ({
                            ...current,
                            valueInput: e.target.value,
                          }))
                        }
                        rows={4}
                        placeholder='{"expected": "value"}'
                        className="w-full rounded-lg border border-gray-300 bg-white px-3 py-2 font-mono text-sm text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
                      />
                    ) : (
                      <input
                        type={row.valueType === "number" ? "number" : "text"}
                        value={row.valueInput}
                        onChange={(e) =>
                          updateRow(row.id, (current) => ({
                            ...current,
                            valueInput: e.target.value,
                          }))
                        }
                        placeholder={
                          row.valueType === "number" ? "42" : "expected value"
                        }
                        className="w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
                      />
                    )}
                  </div>
                </div>
              </div>
            ))
          )}

          <button
            type="button"
            onClick={addCondition}
            className="inline-flex items-center gap-2 rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-700 hover:bg-gray-50"
          >
            <Plus className="h-4 w-4" />
            Add condition
          </button>
        </div>
      ) : (
        <textarea
          value={rawText}
          onChange={(e) => setRawText(e.target.value)}
          rows={10}
          placeholder={`[\n  {\n    "field": "status",\n    "operator": "equals",\n    "value": "error"\n  }\n]`}
          className="w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
        />
      )}

      {currentError && <p className="text-sm text-red-600">{currentError}</p>}
    </div>
  );
}
