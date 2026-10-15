/* eslint-disable react-refresh/only-export-components -- shared flat-schema utilities */
import ContractValueForm from "./ContractValueForm";

/** A JSON-compatible value that can appear in form data. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type JsonValue = any;

export interface ParamSchemaProperty {
  type?: string | string[];
  description?: string;
  default?: JsonValue;
  enum?: string[];
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  secret?: boolean;
  required?: boolean | string[];
  position?: number;
  items?: Record<string, unknown>;
}

export interface ParamSchema {
  [key: string]: ParamSchemaProperty;
}

/** Extract definitions from Attune's flat per-field schema document. */
export function extractProperties(
  schema: ParamSchema | Record<string, unknown> | null | undefined,
): Record<string, ParamSchemaProperty> {
  if (!schema || typeof schema !== "object") return {};
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
  contract?: unknown;
  onPreviewDigest?: (digest: string | undefined) => void;
  values: Record<string, JsonValue>;
  onChange: (values: Record<string, JsonValue>) => void;
  errors?: Record<string, string>;
  disabled?: boolean;
  className?: string;
  allowTemplates?: boolean;
  hideTemplateHint?: boolean;
  templateNamespace?: string;
}

/** All flat forms use a stored contract or a server-resolved authoring preview. */
export default function ParamSchemaForm({
  schema,
  contract,
  onPreviewDigest,
  values,
  onChange,
  errors = {},
  disabled = false,
  className = "",
  allowTemplates = false,
  templateNamespace = "event.payload",
}: ParamSchemaFormProps) {
  return (
    <div className={className}>
      <ContractValueForm
        schema={schema}
        contract={contract}
        onPreviewDigest={onPreviewDigest}
        value={values}
        disabled={disabled}
        allowTemplates={allowTemplates}
        templateNamespace={templateNamespace}
        onChange={(next) => {
          if (typeof next === "object" && next !== null && !Array.isArray(next))
            onChange(Object.fromEntries(Object.entries(next)));
        }}
      />
      {Object.entries(errors).map(([path, message]) => (
        <p key={path} role="alert" className="text-sm text-red-600">
          {path}: {message}
        </p>
      ))}
    </div>
  );
}

function isTemplateExpression(value: JsonValue): boolean {
  return typeof value === "string" && /\{\{.*\}\}/.test(value);
}

/** Advisory literal checks. The server validates rendered values at acceptance. */
export function validateParamSchema(
  schema: ParamSchema,
  values: Record<string, JsonValue>,
  allowTemplates: boolean = false,
): Record<string, string> {
  const errors: Record<string, string> = {};
  const properties = extractProperties(schema);
  Object.entries(properties).forEach(([key, param]) => {
    if (param?.required === true && values[key] === undefined)
      errors[key] = "This field is required";
  });
  Object.entries(properties).forEach(([key, param]) => {
    const value = values[key];
    if (value === undefined || (allowTemplates && isTemplateExpression(value)))
      return;
    const type = param?.type || "string";
    switch (type) {
      case "number":
      case "integer":
        if (typeof value !== "number" && isNaN(Number(value))) {
          if (allowTemplates) break;
          errors[key] = `Must be a valid ${type}`;
        } else {
          const numValue = typeof value === "number" ? value : Number(value);
          if (param?.minimum !== undefined && numValue < param.minimum)
            errors[key] = `Must be at least ${param.minimum}`;
          if (param?.maximum !== undefined && numValue > param.maximum)
            errors[key] = `Must be at most ${param.maximum}`;
        }
        break;
      case "boolean":
        break;
      case "array":
        if (!Array.isArray(value)) {
          if (allowTemplates && typeof value === "string") break;
          try {
            JSON.parse(value);
          } catch {
            errors[key] = "Must be a valid array (JSON format)";
          }
        }
        break;
      case "object":
        if (typeof value !== "object" || Array.isArray(value)) {
          if (allowTemplates && typeof value === "string") break;
          try {
            const parsed = JSON.parse(value);
            if (typeof parsed !== "object" || Array.isArray(parsed))
              errors[key] = "Must be a valid object (JSON format)";
          } catch {
            errors[key] = "Must be a valid object (JSON format)";
          }
        }
        break;
      case "string":
        if (typeof value === "string") {
          if (param?.minLength !== undefined && value.length < param.minLength)
            errors[key] = `Must be at least ${param.minLength} characters`;
          if (param?.maxLength !== undefined && value.length > param.maxLength)
            errors[key] = `Must be at most ${param.maxLength} characters`;
        }
        break;
    }
    if (
      !allowTemplates &&
      param?.enum &&
      param.enum.length > 0 &&
      !param.enum.includes(value)
    )
      errors[key] = `Must be one of: ${param.enum.join(", ")}`;
  });
  return errors;
}
