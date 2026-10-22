import { useEffect, useState } from "react";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import { SchemaPreviewContext } from "@/api/models/SchemaPreviewContext";

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? Object.fromEntries(Object.entries(value))
    : undefined;
}

function resolve(
  node: unknown,
  definitions: Record<string, unknown>,
  depth = 0,
): Record<string, unknown> | undefined {
  const schema = record(node);
  if (!schema || depth > 32) return undefined;
  if (typeof schema.$ref !== "string") return schema;
  if (!schema.$ref.startsWith("#/$defs/")) return undefined;
  const target = resolve(
    definitions[schema.$ref.slice(8)],
    definitions,
    depth + 1,
  );
  return target
    ? {
        ...target,
        ...Object.fromEntries(
          Object.entries(schema).filter(([key]) => key !== "$ref"),
        ),
        ...(target.secret === true || schema.secret === true
          ? { secret: true }
          : {}),
      }
    : undefined;
}

function initial(schema: Record<string, unknown>): unknown {
  if (Object.hasOwn(schema, "default")) return schema.default;
  switch (schema.type) {
    case "object":
      return {};
    case "array":
      return [];
    case "boolean":
      return false;
    case "integer":
    case "number":
      return 0;
    default:
      return "";
  }
}

function JsonInput({
  value,
  onChange,
  disabled,
  label,
}: {
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
  label: string;
}) {
  const [edit, setEdit] = useState({
    source: value,
    text: JSON.stringify(value) ?? "",
    invalid: false,
  });
  const text = Object.is(edit.source, value)
    ? edit.text
    : (JSON.stringify(value) ?? "");
  const invalid = Object.is(edit.source, value) && edit.invalid;
  return (
    <div>
      <p className="text-xs text-gray-500">
        Edit JSON for constraints without structured controls. The server
        validates the complete value. Secret-bearing JSON stays masked.
      </p>
      <input
        aria-label={label || "Parameters JSON"}
        aria-invalid={invalid}
        ref={(input) =>
          input?.setCustomValidity(
            invalid ? "Enter valid JSON before saving." : "",
          )
        }
        type="password"
        value={text}
        disabled={disabled}
        className="w-full border rounded p-2"
        onChange={(event) => {
          const text = event.target.value;
          try {
            const parsed: unknown = JSON.parse(text);
            setEdit({ source: parsed, text, invalid: false });
            onChange(parsed);
          } catch {
            setEdit({ source: value, text, invalid: true });
          }
        }}
      />
      {invalid && (
        <p role="alert" className="text-red-600 text-sm">
          Enter valid JSON before saving.
        </p>
      )}
    </div>
  );
}

type NodeProps = {
  node: unknown;
  definitions: Record<string, unknown>;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
  templates: boolean;
  templateNamespace?: string;
  path: string;
  depth?: number;
  secret?: boolean;
  budget?: number;
  required?: boolean;
  allowNodeTemplate?: boolean;
};

function Node({
  node,
  definitions,
  value,
  onChange,
  disabled,
  templates,
  templateNamespace = "event.payload",
  path,
  depth = 0,
  secret = false,
  budget = 200,
  required: isRequired = false,
  allowNodeTemplate = true,
}: NodeProps) {
  const schema = resolve(node, definitions);
  secret ||= schema?.secret === true;
  if (
    !schema ||
    depth >= 12 ||
    ["allOf", "anyOf", "oneOf", "not", "contains", "prefixItems"].some(
      (key) => key in schema,
    )
  ) {
    return (
      <JsonInput
        value={value}
        onChange={onChange}
        disabled={disabled}
        label={path}
      />
    );
  }
  const template =
    templates && allowNodeTemplate ? (
      <button
        type="button"
        disabled={disabled}
        className="text-xs text-blue-600"
        onClick={() =>
          onChange(
            secret
              ? "{{ keystore['system.key'] }}"
              : `{{ ${templateNamespace}.value }}`,
          )
        }
      >
        Use template for {path || "value"}
      </button>
    ) : null;
  if (templates && typeof value === "string" && value.includes("{{")) {
    return (
      <input
        aria-label={path}
        type={secret ? "password" : "text"}
        value={value}
        disabled={disabled}
        className="w-full border rounded p-2"
        onChange={(event) => onChange(event.target.value)}
      />
    );
  }
  const choices: unknown[] | undefined = Array.isArray(schema.enum)
    ? schema.enum
    : undefined;
  if (
    !secret &&
    choices &&
    choices.length > 0 &&
    choices.length <= 100 &&
    choices.every(
      (choice) =>
        choice === null ||
        typeof choice === "string" ||
        typeof choice === "boolean" ||
        (typeof choice === "number" && Number.isFinite(choice)),
    )
  ) {
    // Option indices preserve JSON types, including false, zero, null, and "".
    const selected = choices.findIndex((choice) => choice === value);
    return (
      <div>
        {template}
        <select
          aria-label={path || "Value"}
          required={isRequired}
          disabled={disabled}
          value={selected < 0 ? "" : String(selected)}
          className="w-full border rounded p-2"
          onChange={(event) => {
            const index = Number(event.target.value);
            if (
              event.target.value !== "" &&
              Number.isInteger(index) &&
              Object.hasOwn(choices, index)
            )
              onChange(choices[index]);
          }}
        >
          <option value="" disabled>
            {value === undefined
              ? "Select a value"
              : "Current value is not an allowed choice"}
          </option>
          {choices.map((choice, index) => (
            <option key={index} value={String(index)}>
              {typeof choice === "string"
                ? choice === ""
                  ? "(empty string)"
                  : choice
                : JSON.stringify(choice)}
            </option>
          ))}
        </select>
      </div>
    );
  }
  if (schema.type === "object") {
    const values = record(value);
    const properties = record(schema.properties);
    if (!values)
      return (
        <div>
          <button
            type="button"
            disabled={disabled}
            onClick={() => onChange({})}
          >
            Create object value
          </button>
          <JsonInput
            value={value}
            onChange={onChange}
            disabled={disabled}
            label={path}
          />
        </div>
      );
    if (
      !values ||
      !properties ||
      Object.keys(properties).length > Math.min(100, budget - 1) ||
      Object.keys(values).some((key) => !Object.hasOwn(properties, key))
    ) {
      return (
        <JsonInput
          value={value}
          onChange={onChange}
          disabled={disabled}
          label={path}
        />
      );
    }
    const required = Array.isArray(schema.required) ? schema.required : [];
    const fields = Object.entries(properties)
      .map(([key, child], order) => {
        const annotations = resolve(child, definitions);
        return {
          key,
          child,
          type: annotations?.type,
          order,
          title:
            typeof annotations?.title === "string" && annotations.title.trim()
              ? annotations.title
              : key,
          description:
            typeof annotations?.description === "string"
              ? annotations.description
              : undefined,
          position:
            typeof annotations?.position === "number"
              ? annotations.position
              : Infinity,
        };
      })
      .sort(
        (left, right) =>
          left.position - right.position || left.order - right.order,
      );
    return (
      <fieldset className="space-y-3 border rounded p-3">
        <legend>
          {typeof schema.title === "string" && schema.title.trim()
            ? schema.title
            : path || "Parameters"}
        </legend>
        {template}
        {fields.map(({ key, child, type, title, description }) => (
          <div key={key} className="space-y-1">
            <span className="block text-sm font-medium">
              {title}
              {required.includes(key) ? " *" : ""}
            </span>
            {title !== key && (
              <span className="text-xs font-mono text-gray-500">{key}</span>
            )}
            {description && (
              <p className="text-xs text-gray-500">{description}</p>
            )}
            {!Object.hasOwn(values, key) && !required.includes(key) ? (
              <button
                type="button"
                disabled={disabled}
                className="text-sm text-blue-600"
                onClick={() =>
                  onChange({
                    ...values,
                    [key]: initial(resolve(child, definitions) ?? {}),
                  })
                }
              >
                Add {title}
              </button>
            ) : (
              <>
                {!Object.hasOwn(values, key) &&
                  (type === "object" || type === "array") && (
                    <button
                      type="button"
                      disabled={disabled}
                      className="text-sm text-blue-600"
                      onClick={() =>
                        onChange({
                          ...values,
                          [key]: type === "object" ? {} : [],
                        })
                      }
                    >
                      Use empty {type === "object" ? "object" : "list"} for{" "}
                      {title}
                    </button>
                  )}
                <Node
                  node={child}
                  definitions={definitions}
                  value={
                    Object.hasOwn(values, key)
                      ? values[key]
                      : type === "object"
                        ? {}
                        : type === "array"
                          ? []
                          : undefined
                  }
                  required={required.includes(key)}
                  disabled={disabled}
                  templates={templates}
                  templateNamespace={templateNamespace}
                  secret={secret}
                  budget={Math.floor(
                    (budget - 1) / Math.max(1, Object.keys(properties).length),
                  )}
                  path={`${path}/${key.replaceAll("~", "~0").replaceAll("/", "~1")}`}
                  depth={depth + 1}
                  onChange={(next) => onChange({ ...values, [key]: next })}
                />
                {!required.includes(key) && (
                  <button
                    type="button"
                    disabled={disabled}
                    className="text-xs text-gray-500"
                    onClick={() =>
                      onChange(
                        Object.fromEntries(
                          Object.entries(values).filter(
                            ([name]) => name !== key,
                          ),
                        ),
                      )
                    }
                  >
                    Remove {title}
                  </button>
                )}
              </>
            )}
          </div>
        ))}
      </fieldset>
    );
  }
  if (schema.type === "array") {
    if (!Array.isArray(value))
      return (
        <div>
          <button
            type="button"
            disabled={disabled}
            onClick={() => onChange([])}
          >
            Create list value
          </button>
          <JsonInput
            value={value}
            onChange={onChange}
            disabled={disabled}
            label={path}
          />
        </div>
      );
    if (!Array.isArray(value) || value.length > Math.min(100, budget - 1))
      return (
        <JsonInput
          value={value}
          onChange={onChange}
          disabled={disabled}
          label={path}
        />
      );
    return (
      <fieldset className="space-y-2 border rounded p-3">
        <legend>{path}</legend>
        {template}
        {value.map((item: unknown, index: number) => (
          <div key={index}>
            <Node
              node={schema.items}
              definitions={definitions}
              value={item}
              disabled={disabled}
              templates={templates}
              templateNamespace={templateNamespace}
              secret={secret}
              budget={Math.floor((budget - 1) / Math.max(1, value.length))}
              path={`${path}/${index}`}
              depth={depth + 1}
              onChange={(next) =>
                onChange(
                  value.map((current: unknown, i: number) =>
                    i === index ? next : current,
                  ),
                )
              }
            />
            <button
              type="button"
              disabled={disabled}
              onClick={() =>
                onChange(value.filter((_: unknown, i: number) => i !== index))
              }
            >
              Remove item {index + 1}
            </button>
          </div>
        ))}
        <button
          type="button"
          disabled={disabled || value.length >= 100}
          onClick={() =>
            onChange([
              ...value,
              initial(resolve(schema.items, definitions) ?? {}),
            ])
          }
        >
          Add item
        </button>
      </fieldset>
    );
  }
  if (
    value === null ||
    typeof schema.type !== "string" ||
    !["string", "integer", "number", "boolean"].includes(schema.type)
  ) {
    return (
      <JsonInput
        value={value}
        onChange={onChange}
        disabled={disabled}
        label={path}
      />
    );
  }
  if (schema.type === "boolean" && secret)
    return (
      <JsonInput
        value={value}
        onChange={onChange}
        disabled={disabled}
        label={path}
      />
    );
  if (schema.type === "boolean" && isRequired)
    return (
      <div>
        {template}
        <select
          aria-label={path}
          required
          disabled={disabled}
          value={typeof value === "boolean" ? String(value) : ""}
          className="w-full border rounded p-2"
          onChange={(event) => {
            if (event.target.value !== "")
              onChange(event.target.value === "true");
          }}
        >
          <option value="" disabled>
            Select a value
          </option>
          <option value="true">True</option>
          <option value="false">False</option>
        </select>
      </div>
    );
  if (schema.type === "boolean")
    return (
      <div>
        {template}
        <input
          aria-label={path}
          type="checkbox"
          checked={value === true}
          disabled={disabled}
          onChange={(event) => onChange(event.target.checked)}
        />
      </div>
    );
  const numeric = schema.type === "integer" || schema.type === "number";
  const constraints = Object.fromEntries(
    Object.entries(schema).filter(
      ([key]) =>
        !(secret && key === "enum") &&
        [
          "enum",
          "minimum",
          "maximum",
          "exclusiveMinimum",
          "exclusiveMaximum",
          "minLength",
          "maxLength",
          "pattern",
          "format",
        ].includes(key),
    ),
  );
  return (
    <div>
      {template}
      {Object.keys(constraints).length > 0 && (
        <p className="text-xs text-gray-500">
          Server constraints: {JSON.stringify(constraints)}
        </p>
      )}
      <input
        aria-label={path}
        type={secret ? "password" : numeric ? "number" : "text"}
        required={
          isRequired &&
          (numeric ||
            (typeof schema.minLength === "number" && schema.minLength > 0))
        }
        min={typeof schema.minimum === "number" ? schema.minimum : undefined}
        max={typeof schema.maximum === "number" ? schema.maximum : undefined}
        step={schema.type === "integer" ? 1 : numeric ? "any" : undefined}
        minLength={
          typeof schema.minLength === "number" ? schema.minLength : undefined
        }
        maxLength={
          typeof schema.maxLength === "number" ? schema.maxLength : undefined
        }
        value={
          typeof value === "string" || typeof value === "number" ? value : ""
        }
        disabled={disabled}
        className="w-full border rounded p-2"
        onChange={(event) => {
          const text = event.target.value;
          onChange(
            numeric && text !== "" && Number.isFinite(Number(text))
              ? Number(text)
              : text,
          );
        }}
      />
    </div>
  );
}

export default function ContractValueForm({
  schema,
  contract,
  value,
  onChange,
  disabled = false,
  allowTemplates = false,
  templateNamespace = "event.payload",
  context = SchemaPreviewContext.FLAT_DOCUMENT,
  maskValue = false,
  onPreviewDigest,
}: {
  schema?: unknown;
  contract?: unknown;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled?: boolean;
  allowTemplates?: boolean;
  templateNamespace?: string;
  context?: SchemaPreviewContext;
  maskValue?: boolean;
  onPreviewDigest?: (digest: string | undefined) => void;
}) {
  const [resolution, setResolution] = useState<
    | {
        schema: unknown;
        context: SchemaPreviewContext;
        kind: "resolved";
        contract: unknown;
      }
    | { schema: unknown; context: SchemaPreviewContext; kind: "failed" }
    | undefined
  >();
  useEffect(() => {
    if (contract !== undefined) return;
    onPreviewDigest?.(undefined);
    let current = true;
    const request = SchemaTypesService.previewSchema({
      requestBody: { context, schema },
    });
    request
      .then((response) => {
        if (current) {
          onPreviewDigest?.(response.data.semantic_digest);
          setResolution({
            schema,
            context,
            kind: "resolved",
            contract: response.data.contract,
          });
        }
      })
      .catch(() => {
        if (current) setResolution({ schema, context, kind: "failed" });
      });
    return () => {
      current = false;
      request.cancel();
    };
  }, [schema, contract, context, onPreviewDigest]);
  const resolved =
    contract !== undefined
      ? contract
      : resolution !== undefined &&
          resolution.schema === schema &&
          resolution.context === context &&
          resolution.kind === "resolved"
        ? resolution.contract
        : undefined;
  const status =
    contract !== undefined
      ? "Stored contract cannot be rendered. Edit JSON; the server still validates its original constraints."
      : resolution !== undefined &&
          resolution.schema === schema &&
          resolution.context === context &&
          resolution.kind === "failed"
        ? "Schema resolution failed. References and constraints cannot be verified here."
        : "Resolving schema...";
  const bundle = record(resolved);
  const root = record(bundle?.annotated_schema);
  if (!root)
    return (
      <div>
        <p role="status" className="text-amber-700 text-sm">
          {status}
        </p>
        <JsonInput
          value={value}
          onChange={onChange}
          disabled={disabled || status === "Resolving schema..."}
          label="Parameters JSON"
        />
      </div>
    );
  const definitions = record(root.$defs) ?? {};
  const annotations = resolve(root, definitions);
  return (
    <div className="space-y-2">
      {annotations?.type !== "object" &&
        typeof annotations?.title === "string" && (
          <p className="text-sm font-medium">{annotations.title}</p>
        )}
      {typeof annotations?.description === "string" && (
        <p className="text-xs text-gray-500">{annotations.description}</p>
      )}
      <Node
        node={root}
        definitions={definitions}
        value={value}
        onChange={onChange}
        disabled={disabled}
        templates={allowTemplates}
        templateNamespace={templateNamespace}
        allowNodeTemplate={context !== SchemaPreviewContext.FLAT_DOCUMENT}
        secret={maskValue}
        path=""
      />
    </div>
  );
}
