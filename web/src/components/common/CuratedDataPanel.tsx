import { useState } from "react";
import { type ParamSchemaProperty } from "@/components/common/ParamSchemaForm";
import {
  isJsonObject,
  sortedSchemaEntries,
} from "@/components/common/curatedDataUtils";

function stringifyValue(value: unknown): string {
  if (value === null) return "null";
  if (value === undefined) return "Not provided";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  return JSON.stringify(value, null, 2);
}

function displayType(schema?: ParamSchemaProperty, value?: unknown): string {
  if (schema?.type) return schema.type;
  if (Array.isArray(value)) return "array";
  if (value === null) return "null";
  return typeof value === "undefined" ? "unknown" : typeof value;
}

export function JsonValueDisplay({ value }: { value: unknown }) {
  if (isJsonObject(value) || Array.isArray(value)) {
    return (
      <pre className="mt-1 bg-gray-50 border border-gray-200 rounded p-2 text-xs overflow-x-auto whitespace-pre-wrap">
        {JSON.stringify(value, null, 2)}
      </pre>
    );
  }

  return (
    <span className="text-sm text-gray-900 break-words">
      {stringifyValue(value)}
    </span>
  );
}

export function SchemaValueRows({
  schema,
  values,
  emptyMessage,
  maskSecrets = false,
  hideUnprovided = false,
}: {
  schema: unknown;
  values: unknown;
  emptyMessage: string;
  maskSecrets?: boolean;
  /** When true, schema params with no provided value are hidden by default with a toggle to show all. */
  hideUnprovided?: boolean;
}) {
  const [showAll, setShowAll] = useState(false);
  const schemaEntries = sortedSchemaEntries(schema);
  const valueObject = isJsonObject(values) ? values : {};
  const renderedKeys = new Set(schemaEntries.map(([key]) => key));
  const extraEntries = Object.entries(valueObject).filter(
    ([key]) => !renderedKeys.has(key),
  );

  // When hideUnprovided is active, filter out schema entries with no value
  const filteredSchemaEntries =
    hideUnprovided && !showAll
      ? schemaEntries.filter(
          ([key]) =>
            Object.prototype.hasOwnProperty.call(valueObject, key) &&
            valueObject[key] !== undefined &&
            valueObject[key] !== null,
        )
      : schemaEntries;

  const hiddenCount = schemaEntries.length - filteredSchemaEntries.length;

  if (schemaEntries.length === 0 && extraEntries.length === 0) {
    return <p className="text-sm text-gray-500">{emptyMessage}</p>;
  }

  return (
    <div>
      <div className="divide-y divide-gray-100 rounded-lg border border-gray-200">
        {filteredSchemaEntries.map(([key, field]) => {
          const hasValue = Object.prototype.hasOwnProperty.call(
            valueObject,
            key,
          );
          const value = hasValue ? valueObject[key] : undefined;
          return (
            <div key={key} className="p-3">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-sm font-medium text-gray-900">
                  {key}
                </span>
                <span className="rounded bg-gray-100 px-2 py-0.5 text-xs text-gray-600">
                  {displayType(field, value)}
                </span>
                {field.required && (
                  <span className="rounded bg-blue-50 px-2 py-0.5 text-xs text-blue-700">
                    required
                  </span>
                )}
                {field.secret && (
                  <span className="rounded bg-purple-50 px-2 py-0.5 text-xs text-purple-700">
                    secret
                  </span>
                )}
              </div>
              {field.description && (
                <p className="mt-1 text-sm text-gray-500">
                  {field.description}
                </p>
              )}
              <div className="mt-2">
                {maskSecrets && field.secret && hasValue ? (
                  <span className="text-sm text-gray-500">••••••••</span>
                ) : (
                  <JsonValueDisplay value={value} />
                )}
              </div>
            </div>
          );
        })}

        {extraEntries.map(([key, value]) => (
          <div key={key} className="p-3">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-sm font-medium text-gray-900">
                {key}
              </span>
              <span className="rounded bg-gray-100 px-2 py-0.5 text-xs text-gray-600">
                {displayType(undefined, value)}
              </span>
              <span className="rounded bg-yellow-50 px-2 py-0.5 text-xs text-yellow-700">
                not in schema
              </span>
            </div>
            <div className="mt-2">
              <JsonValueDisplay value={value} />
            </div>
          </div>
        ))}
      </div>

      {hideUnprovided && hiddenCount > 0 && (
        <button
          onClick={() => setShowAll(!showAll)}
          className="mt-2 text-xs text-blue-600 hover:text-blue-800"
        >
          {showAll
            ? "Hide unprovided parameters"
            : `Show ${hiddenCount} unprovided parameter${hiddenCount !== 1 ? "s" : ""}`}
        </button>
      )}
    </div>
  );
}

export function CuratedDataCard({
  title,
  description,
  schema,
  values,
  emptyMessage,
  maskSecrets = false,
}: {
  title: string;
  description?: string;
  schema?: unknown;
  values: unknown;
  emptyMessage: string;
  maskSecrets?: boolean;
}) {
  return (
    <div className="bg-white rounded-lg shadow">
      <div className="px-6 py-4 border-b border-gray-200">
        <h2 className="text-lg font-semibold text-gray-900">{title}</h2>
        {description && (
          <p className="text-sm text-gray-600 mt-1">{description}</p>
        )}
      </div>
      <div className="px-6 py-4">
        <SchemaValueRows
          schema={schema}
          values={values}
          emptyMessage={emptyMessage}
          maskSecrets={maskSecrets}
        />
      </div>
    </div>
  );
}
