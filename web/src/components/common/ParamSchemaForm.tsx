/* eslint-disable react-refresh/only-export-components -- extractProperties and validateParamSchema are shared utilities co-located with the form component */
import { useState, useEffect } from "react";
import SearchableSelect from "@/components/common/SearchableSelect";

/** A JSON-compatible value that can appear in form data */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type JsonValue = any;

/**
 * StackStorm-style parameter schema format.
 * Parameters are defined as a flat map of parameter name to definition,
 * with `required` and `secret` inlined per-parameter.
 *
 * Example:
 * {
 *   "url": { "type": "string", "description": "Target URL", "required": true },
 *   "token": { "type": "string", "secret": true }
 * }
 */
export interface ParamSchemaProperty {
  type?: "string" | "number" | "integer" | "boolean" | "array" | "object";
  description?: string;
  default?: JsonValue;
  enum?: string[];
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  secret?: boolean;
  required?: boolean;
  position?: number;
  items?: Record<string, unknown>;
}

type EnumOption = {
  value: string;
  label: string;
};

export interface ParamSchema {
  [key: string]: ParamSchemaProperty;
}

/**
 * Props for ParamSchemaForm component
 */
/**
 * Extract the parameter properties from a flat parameter schema.
 *
 * All schemas (param_schema, out_schema, conf_schema) use the same flat format:
 * { param_name: { type, description, required, secret, ... }, ... }
 */
export function extractProperties(
  schema: ParamSchema | Record<string, unknown> | null | undefined,
): Record<string, ParamSchemaProperty> {
  if (!schema || typeof schema !== "object") return {};
  // StackStorm-style flat format: { param_name: { type, description, required, ... }, ... }
  // Filter out entries that don't look like parameter definitions (e.g., stray "type" or "required" keys)
  const props: Record<string, ParamSchemaProperty> = {};
  for (const [key, value] of Object.entries(schema)) {
    if (value && typeof value === "object" && !Array.isArray(value)) {
      props[key] = value as ParamSchemaProperty;
    }
  }
  return props;
}

interface ParamSchemaFormProps {
  schema: ParamSchema;
  values: Record<string, JsonValue>;
  onChange: (values: Record<string, JsonValue>) => void;
  errors?: Record<string, string>;
  disabled?: boolean;
  className?: string;
  /**
   * When true, all inputs render as text fields that accept template expressions
   * like {{ event.payload.field }}, {{ pack.config.key }}, {{ system.timestamp }}.
   * Used in rule configuration where parameters may be dynamically resolved
   * at enforcement time rather than set to literal values.
   */
  allowTemplates?: boolean;
  /**
   * When true, hides the amber "Template expressions" info banner at the top
   * while still enabling template-mode inputs. Useful when template syntax is
   * supported but the contextual hint (rule-specific namespaces) doesn't apply.
   */
  hideTemplateHint?: boolean;
  /**
   * Namespace used for template-mode placeholders. Defaults to the rule event
   * payload namespace used by existing rule forms.
   */
  templateNamespace?: string;
}

/**
 * Check if a string value contains a template expression ({{ ... }})
 */
function isTemplateExpression(value: JsonValue): boolean {
  return typeof value === "string" && /\{\{.*\}\}/.test(value);
}

/**
 * Format a value for display in a text input.
 * Non-string values (booleans, numbers, objects, arrays) are JSON-stringified
 * so the user can edit them as text.
 */
function valueToString(value: JsonValue): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

/**
 * Attempt to parse a text input value back to the appropriate JS type.
 * Template expressions are always kept as strings.
 * Plain values are coerced to the schema type when possible.
 */
function parseTemplateValue(raw: string, type: string): JsonValue {
  if (raw === "") return "";
  // Template expressions stay as strings - resolved server-side
  if (isTemplateExpression(raw)) return raw;

  switch (type) {
    case "boolean":
      if (raw === "true") return true;
      if (raw === "false") return false;
      return raw; // keep as string if not a recognised literal
    case "number":
      if (!isNaN(Number(raw))) return parseFloat(raw);
      return raw;
    case "integer":
      if (!isNaN(Number(raw)) && Number.isInteger(Number(raw)))
        return parseInt(raw, 10);
      return raw;
    case "array":
    case "object":
      try {
        return JSON.parse(raw);
      } catch {
        return raw;
      }
    default:
      return raw;
  }
}

/**
 * Dynamic form component that renders inputs based on a parameter schema.
 * Supports standard JSON Schema format with properties and required array.
 * Supports string, number, integer, boolean, array, object, and enum types.
 *
 * When `allowTemplates` is enabled, every field renders as a text input that
 * accepts Jinja2-style template expressions (e.g. {{ event.payload.x }}).
 * This is essential for rule configuration, where parameter values may reference
 * event payloads, pack configs, keys, or system variables.
 */
function EnumTextField({
  value,
  onChange,
  options,
  placeholder,
  disabled,
}: {
  value: string;
  onChange: (value: string) => void;
  options: EnumOption[];
  placeholder?: string;
  disabled?: boolean;
}) {
  return (
    <SearchableSelect
      value={options.some((opt) => opt.value === value) ? value : ""}
      onChange={(selected) => onChange(String(selected))}
      options={options}
      placeholder={placeholder || "Select..."}
      disabled={disabled}
    />
  );
}

export default function ParamSchemaForm({
  schema,
  values,
  onChange,
  errors = {},
  disabled = false,
  className = "",
  allowTemplates = false,
  hideTemplateHint = false,
  templateNamespace = "event.payload",
}: ParamSchemaFormProps) {
  const [localErrors, setLocalErrors] = useState<Record<string, string>>({});

  // Merge external and local errors
  const allErrors = { ...localErrors, ...errors };

  const properties = extractProperties(schema);

  // Initialize values with defaults from schema
  useEffect(() => {
    const initialValues = Object.entries(properties).reduce(
      (acc, [key, param]) => {
        if (values[key] === undefined && param?.default !== undefined) {
          acc[key] = param.default;
        }
        return acc;
      },
      { ...values } as Record<string, JsonValue>,
    );

    // Only update if there are new defaults
    if (JSON.stringify(initialValues) !== JSON.stringify(values)) {
      onChange(initialValues);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [schema]); // Only run when schema changes

  /**
   * Handle input change for a specific field
   */
  const handleInputChange = (key: string, value: JsonValue) => {
    const newValues = { ...values, [key]: value };
    onChange(newValues);

    // Clear error for this field
    if (allErrors[key]) {
      setLocalErrors((prev) => {
        const updated = { ...prev };
        delete updated[key];
        return updated;
      });
    }
  };

  /**
   * Check if a field is required
   */
  const isRequired = (key: string): boolean => {
    return !!properties[key]?.required;
  };

  /**
   * Get a placeholder hint for template-mode inputs
   */
  const getTemplatePlaceholder = (
    key: string,
    param: ParamSchemaProperty | undefined,
  ) => {
    const type = param?.type || "string";
    const example = `{{ ${templateNamespace}.${key} }}`;
    switch (type) {
      case "boolean":
        return `true, false, or ${example}`;
      case "number":
      case "integer":
        return `${type} value or ${example}`;
      case "array":
        return `["a","b"] or ${example}`;
      case "object":
        return `{"k":"v"} or ${example}`;
      default:
        if (param?.enum && param.enum.length > 0) {
          const options = param.enum.slice(0, 3).join(", ");
          const suffix = param.enum.length > 3 ? ", ..." : "";
          return `${options}${suffix} or ${example}`;
        }
        return param?.description || example;
    }
  };

  /**
   * Render a template-mode text input for any parameter type
   */
  const renderTemplateInput = (
    key: string,
    param: ParamSchemaProperty | undefined,
  ) => {
    const type = param?.type || "string";
    const rawValue = values[key] ?? param?.default ?? "";
    const isDisabled = disabled;
    const displayValue = valueToString(rawValue);

    // Use a textarea for complex types (array/object) to give more room
    if (type === "array" || type === "object") {
      return (
        <textarea
          value={displayValue}
          onChange={(e) =>
            handleInputChange(key, parseTemplateValue(e.target.value, type))
          }
          disabled={isDisabled}
          rows={3}
          className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 font-mono text-sm disabled:bg-gray-100 disabled:cursor-not-allowed"
          placeholder={getTemplatePlaceholder(key, param)}
        />
      );
    }

    // In template mode, enum fields should stay as free text while still
    // offering searchable suggestions for literal values.
    if (param?.enum && param.enum.length > 0) {
      return (
        <EnumTextField
          value={displayValue}
          onChange={(nextValue) =>
            handleInputChange(key, parseTemplateValue(nextValue, type))
          }
          options={param.enum.map((option: string) => ({
            value: option,
            label: option,
          }))}
          placeholder={getTemplatePlaceholder(key, param)}
          disabled={isDisabled}
        />
      );
    }

    return (
      <input
        type="text"
        value={displayValue}
        onChange={(e) =>
          handleInputChange(key, parseTemplateValue(e.target.value, type))
        }
        disabled={isDisabled}
        className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-100 disabled:cursor-not-allowed"
        placeholder={getTemplatePlaceholder(key, param)}
      />
    );
  };

  /**
   * Render input field based on parameter type (standard mode)
   */
  const renderInput = (key: string, param: ParamSchemaProperty | undefined) => {
    const type = param?.type || "string";
    const value = values[key] ?? param?.default ?? "";
    const isDisabled = disabled;

    switch (type) {
      case "boolean":
        return (
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={!!value}
              onChange={(e) => handleInputChange(key, e.target.checked)}
              disabled={isDisabled}
              className="w-4 h-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
            />
            <span className="text-sm text-gray-700">
              {param?.description || "Enable"}
            </span>
          </label>
        );

      case "number":
      case "integer":
        return (
          <input
            type="number"
            value={value}
            onChange={(e) =>
              handleInputChange(
                key,
                type === "integer"
                  ? parseInt(e.target.value) || 0
                  : parseFloat(e.target.value) || 0,
              )
            }
            disabled={isDisabled}
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-100 disabled:cursor-not-allowed"
            placeholder={param?.description}
            step={type === "integer" ? "1" : "any"}
            min={param?.minimum}
            max={param?.maximum}
          />
        );

      case "array":
        return (
          <textarea
            value={
              Array.isArray(value) ? JSON.stringify(value, null, 2) : value
            }
            onChange={(e) => {
              try {
                const parsed = JSON.parse(e.target.value);
                handleInputChange(key, parsed);
              } catch {
                // Allow intermediate invalid JSON while typing
                handleInputChange(key, e.target.value);
              }
            }}
            onBlur={() => {
              // Validate on blur
              try {
                if (typeof value === "string") {
                  const parsed = JSON.parse(value);
                  handleInputChange(key, parsed);
                }
              } catch {
                // Invalid JSON - will be caught by validation
              }
            }}
            disabled={isDisabled}
            rows={4}
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 font-mono text-sm disabled:bg-gray-100 disabled:cursor-not-allowed"
            placeholder='["item1", "item2"]'
          />
        );

      case "object":
        return (
          <textarea
            value={
              typeof value === "object" && value !== null
                ? JSON.stringify(value, null, 2)
                : value
            }
            onChange={(e) => {
              try {
                const parsed = JSON.parse(e.target.value);
                handleInputChange(key, parsed);
              } catch {
                // Allow intermediate invalid JSON while typing
                handleInputChange(key, e.target.value);
              }
            }}
            onBlur={() => {
              // Validate on blur
              try {
                if (typeof value === "string") {
                  const parsed = JSON.parse(value);
                  handleInputChange(key, parsed);
                }
              } catch {
                // Invalid JSON - will be caught by validation
              }
            }}
            disabled={isDisabled}
            rows={4}
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 font-mono text-sm disabled:bg-gray-100 disabled:cursor-not-allowed"
            placeholder='{"key": "value"}'
          />
        );

      default:
        // String type - enum fields use the same searchable text pattern
        // as template-enabled fields so users can pick a value or type one.
        if (param?.enum && param.enum.length > 0) {
          return (
            <EnumTextField
              value={String(value)}
              onChange={(nextValue) => handleInputChange(key, nextValue)}
              options={param.enum.map((option: string) => ({
                value: option,
                label: option,
              }))}
              placeholder={param?.description}
              disabled={isDisabled}
            />
          );
        }

        // Default to text input
        return (
          <input
            type={param?.secret ? "password" : "text"}
            value={value}
            onChange={(e) => handleInputChange(key, e.target.value)}
            disabled={isDisabled}
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-100 disabled:cursor-not-allowed"
            placeholder={param?.description}
            minLength={param?.minLength}
            maxLength={param?.maxLength}
          />
        );
    }
  };

  /**
   * Render type hint badge and additional context for template-mode fields
   */
  const renderTemplateHints = (
    _key: string,
    param: ParamSchemaProperty | undefined,
  ) => {
    const type = param?.type || "string";
    const hints: string[] = [];

    if (type === "boolean") {
      hints.push("Accepts: true, false, or a template expression");
    } else if (type === "number" || type === "integer") {
      const parts = [`Accepts: ${type} value`];
      if (param?.minimum !== undefined) parts.push(`min: ${param.minimum}`);
      if (param?.maximum !== undefined) parts.push(`max: ${param.maximum}`);
      hints.push(parts.join(", ") + ", or a template expression");
    } else if (param?.enum && param.enum.length > 0) {
      hints.push(`Options: ${param.enum.join(", ")}`);
      hints.push("Also accepts a template expression");
    }

    if (hints.length === 0) return null;

    return (
      <div className="mt-1 space-y-0.5">
        {hints.map((hint, i) => (
          <p key={i} className="text-xs text-gray-500">
            {hint}
          </p>
        ))}
      </div>
    );
  };

  const paramEntries = Object.entries(properties);

  if (paramEntries.length === 0) {
    return (
      <div className="p-4 bg-gray-50 rounded-lg text-center text-sm text-gray-600">
        No parameters required
      </div>
    );
  }

  return (
    <div className={`space-y-4 ${className}`}>
      {allowTemplates && !hideTemplateHint && (
        <div className="px-3 py-2 bg-amber-50 border border-amber-200 rounded-lg">
          <p className="text-xs text-amber-800">
            <span className="font-semibold">Template expressions</span> are
            supported. Use{" "}
            <code className="px-1 py-0.5 bg-amber-100 rounded text-[11px]">
              {"{{ event.payload.field }}"}
            </code>
            ,{" "}
            <code className="px-1 py-0.5 bg-amber-100 rounded text-[11px]">
              {"{{ pack.config.key }}"}
            </code>
            , or{" "}
            <code className="px-1 py-0.5 bg-amber-100 rounded text-[11px]">
              {"{{ system.timestamp }}"}
            </code>{" "}
            to dynamically resolve values when the rule fires.
          </p>
        </div>
      )}
      {paramEntries.map(([key, param]) => (
        <div key={key}>
          <label className="block mb-2">
            <div className="flex items-center gap-2 mb-1">
              <span className="font-mono font-semibold text-sm">{key}</span>
              {isRequired(key) && (
                <span className="text-xs px-2 py-0.5 bg-red-100 text-red-700 rounded">
                  Required
                </span>
              )}
              <span className="text-xs px-2 py-0.5 bg-gray-100 text-gray-700 rounded">
                {param?.type || "string"}
              </span>
              {param?.secret && (
                <span className="text-xs px-2 py-0.5 bg-yellow-100 text-yellow-700 rounded">
                  Secret
                </span>
              )}
            </div>
            {param?.description && param?.type !== "boolean" && (
              <p className="text-xs text-gray-600 mb-2">{param.description}</p>
            )}
            {/* For boolean in template mode, show description since there's no checkbox label */}
            {param?.description &&
              param?.type === "boolean" &&
              allowTemplates && (
                <p className="text-xs text-gray-600 mb-2">
                  {param.description}
                </p>
              )}
          </label>
          {allowTemplates
            ? renderTemplateInput(key, param)
            : renderInput(key, param)}
          {allowTemplates && renderTemplateHints(key, param)}
          {allErrors[key] && (
            <p className="text-xs text-red-600 mt-1">{allErrors[key]}</p>
          )}
          {param?.default !== undefined &&
            !values[key] &&
            values[key] !== param.default && (
              <p className="text-xs text-gray-500 mt-1">
                Default: {JSON.stringify(param.default)}
              </p>
            )}
        </div>
      ))}
    </div>
  );
}

/**
 * Utility function to validate parameter values against a schema.
 * Supports standard JSON Schema format.
 *
 * When `allowTemplates` is true, template expressions ({{ ... }}) are
 * accepted for any field type and skip type-specific validation.
 */
export function validateParamSchema(
  schema: ParamSchema,
  values: Record<string, JsonValue>,
  allowTemplates: boolean = false,
): Record<string, string> {
  const errors: Record<string, string> = {};
  const properties = extractProperties(schema);

  // Check required fields (inline per-parameter)
  Object.entries(properties).forEach(([key, param]) => {
    if (param?.required) {
      const value = values[key];
      if (value === undefined || value === null || value === "") {
        errors[key] = "This field is required";
      }
    }
  });

  // Type-specific validation
  Object.entries(properties).forEach(([key, param]) => {
    const value = values[key];

    // Skip if no value and not required
    if (
      (value === undefined || value === null || value === "") &&
      !param?.required
    ) {
      return;
    }

    // Template expressions are always valid in template mode
    if (allowTemplates && isTemplateExpression(value)) {
      return;
    }

    const type = param?.type || "string";

    switch (type) {
      case "number":
      case "integer":
        if (typeof value !== "number" && isNaN(Number(value))) {
          if (allowTemplates) {
            // In template mode, non-numeric strings that aren't templates
            // are still allowed — the user might be mid-edit or using a
            // non-standard expression format. Only warn on submission.
            break;
          }
          errors[key] = `Must be a valid ${type}`;
        } else {
          const numValue = typeof value === "number" ? value : Number(value);
          if (param?.minimum !== undefined && numValue < param.minimum) {
            errors[key] = `Must be at least ${param.minimum}`;
          }
          if (param?.maximum !== undefined && numValue > param.maximum) {
            errors[key] = `Must be at most ${param.maximum}`;
          }
        }
        break;

      case "boolean":
        // In template mode, string values like "true"/"false" are fine
        if (
          allowTemplates &&
          typeof value === "string" &&
          (value === "true" || value === "false")
        ) {
          break;
        }
        break;

      case "array":
        if (!Array.isArray(value)) {
          if (allowTemplates && typeof value === "string") {
            // In template mode, strings are acceptable (could be template or JSON)
            break;
          }
          try {
            JSON.parse(value);
          } catch {
            errors[key] = "Must be a valid array (JSON format)";
          }
        }
        break;

      case "object":
        if (typeof value !== "object" || Array.isArray(value)) {
          if (allowTemplates && typeof value === "string") {
            break;
          }
          try {
            const parsed = JSON.parse(value);
            if (typeof parsed !== "object" || Array.isArray(parsed)) {
              errors[key] = "Must be a valid object (JSON format)";
            }
          } catch {
            errors[key] = "Must be a valid object (JSON format)";
          }
        }
        break;

      case "string":
        if (typeof value === "string") {
          if (
            param?.minLength !== undefined &&
            value.length < param.minLength
          ) {
            errors[key] = `Must be at least ${param.minLength} characters`;
          }
          if (
            param?.maxLength !== undefined &&
            value.length > param.maxLength
          ) {
            errors[key] = `Must be at most ${param.maxLength} characters`;
          }
        }
        break;
    }

    // Enum validation — skip in template mode (value may be a template expression
    // or a string that will be resolved at runtime)
    if (!allowTemplates && param?.enum && param.enum.length > 0) {
      if (!param.enum.includes(value)) {
        errors[key] = `Must be one of: ${param.enum.join(", ")}`;
      }
    }
  });

  return errors;
}
