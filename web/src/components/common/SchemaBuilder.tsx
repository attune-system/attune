import { useState, useEffect, useCallback } from "react";
import { Plus, Trash2, ChevronDown, ChevronRight, Code } from "lucide-react";

/** A single property definition within a flat schema object */
interface SchemaPropertyDef {
  type?: string;
  description?: string;
  required?: boolean;
  secret?: boolean;
  default?: unknown;
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  enum?: string[];
  [key: string]: unknown;
}

/** The flat schema format: each key is a parameter name mapped to its definition */
type FlatSchema = Record<string, SchemaPropertyDef>;

interface SchemaProperty {
  name: string;
  type: string;
  description: string;
  required: boolean;
  secret: boolean;
  default?: string;
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  enum?: string[];
}

interface SchemaBuilderProps {
  value: FlatSchema;
  onChange: (schema: FlatSchema) => void;
  label?: string;
  placeholder?: string;
  error?: string;
  className?: string;
  disabled?: boolean;
}

const PROPERTY_TYPES = [
  { value: "string", label: "String" },
  { value: "number", label: "Number" },
  { value: "integer", label: "Integer" },
  { value: "boolean", label: "Boolean" },
  { value: "array", label: "Array" },
  { value: "object", label: "Object" },
];

export default function SchemaBuilder({
  value,
  onChange,
  label,
  placeholder,
  error,
  className = "",
  disabled = false,
}: SchemaBuilderProps) {
  const [properties, setProperties] = useState<SchemaProperty[]>([]);
  const [showRawJson, setShowRawJson] = useState(false);
  const [rawJson, setRawJson] = useState("");
  const [rawJsonError, setRawJsonError] = useState("");
  const [expandedProperties, setExpandedProperties] = useState<Set<number>>(
    new Set(),
  );

  // Initialize properties from schema value
  // Expects StackStorm-style flat format: { param_name: { type, required, secret, ... }, ... }
  /* eslint-disable react-hooks/set-state-in-effect -- initializes editable local state once from the provided schema */
  useEffect(() => {
    if (!value || typeof value !== "object") return;
    const props: SchemaProperty[] = [];

    Object.entries(value).forEach(([name, propDef]) => {
      if (propDef && typeof propDef === "object" && !Array.isArray(propDef)) {
        const def = propDef as SchemaPropertyDef;
        props.push({
          name,
          type: def.type || "string",
          description: def.description || "",
          required: def.required === true,
          secret: def.secret === true,
          default:
            def.default !== undefined ? JSON.stringify(def.default) : undefined,
          minimum: def.minimum,
          maximum: def.maximum,
          minLength: def.minLength,
          maxLength: def.maxLength,
          pattern: def.pattern,
          enum: def.enum,
        });
      }
    });

    if (props.length > 0) {
      setProperties(props);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  /* eslint-enable react-hooks/set-state-in-effect */

  // Build StackStorm-style flat parameter schema
  const buildSchema = useCallback((): FlatSchema => {
    if (properties.length === 0) {
      return {};
    }

    const schema: FlatSchema = {};

    properties.forEach((prop) => {
      const propSchema: SchemaPropertyDef = {
        type: prop.type,
      };

      if (prop.description) {
        propSchema.description = prop.description;
      }

      if (prop.required) {
        propSchema.required = true;
      }

      if (prop.secret) {
        propSchema.secret = true;
      }

      if (prop.default !== undefined && prop.default !== "") {
        try {
          propSchema.default = JSON.parse(prop.default);
        } catch {
          propSchema.default = prop.default;
        }
      }

      // Type-specific constraints
      if (prop.type === "string") {
        if (prop.minLength !== undefined) propSchema.minLength = prop.minLength;
        if (prop.maxLength !== undefined) propSchema.maxLength = prop.maxLength;
        if (prop.pattern) propSchema.pattern = prop.pattern;
        if (prop.enum && prop.enum.length > 0) propSchema.enum = prop.enum;
      }

      if (prop.type === "number" || prop.type === "integer") {
        if (prop.minimum !== undefined) propSchema.minimum = prop.minimum;
        if (prop.maximum !== undefined) propSchema.maximum = prop.maximum;
      }

      schema[prop.name] = propSchema;
    });

    return schema;
  }, [properties]);

  // Update raw JSON when switching to raw view
  /* eslint-disable react-hooks/set-state-in-effect -- raw editor state must reflect the current form when opened */
  useEffect(() => {
    if (showRawJson) {
      setRawJson(JSON.stringify(buildSchema(), null, 2));
      setRawJsonError("");
    }
  }, [showRawJson, buildSchema]);
  /* eslint-enable react-hooks/set-state-in-effect */

  const handlePropertiesChange = (newProperties: SchemaProperty[]) => {
    setProperties(newProperties);
    const schema = buildSchemaFromProperties(newProperties);
    onChange(schema);
  };

  // Build StackStorm-style flat parameter schema from properties array
  const buildSchemaFromProperties = (props: SchemaProperty[]): FlatSchema => {
    if (props.length === 0) {
      return {};
    }

    const schema: FlatSchema = {};

    props.forEach((prop) => {
      const propSchema: SchemaPropertyDef = {
        type: prop.type,
      };

      if (prop.description) {
        propSchema.description = prop.description;
      }

      if (prop.required) {
        propSchema.required = true;
      }

      if (prop.secret) {
        propSchema.secret = true;
      }

      if (prop.default !== undefined && prop.default !== "") {
        try {
          propSchema.default = JSON.parse(prop.default);
        } catch {
          propSchema.default = prop.default;
        }
      }

      if (prop.type === "string") {
        if (prop.minLength !== undefined) propSchema.minLength = prop.minLength;
        if (prop.maxLength !== undefined) propSchema.maxLength = prop.maxLength;
        if (prop.pattern) propSchema.pattern = prop.pattern;
        if (prop.enum && prop.enum.length > 0) propSchema.enum = prop.enum;
      }

      if (prop.type === "number" || prop.type === "integer") {
        if (prop.minimum !== undefined) propSchema.minimum = prop.minimum;
        if (prop.maximum !== undefined) propSchema.maximum = prop.maximum;
      }

      schema[prop.name] = propSchema;
    });

    return schema;
  };

  const addProperty = () => {
    const newProp: SchemaProperty = {
      name: `param${properties.length + 1}`,
      type: "string",
      description: "",
      required: false,
      secret: false,
    };
    const newIndex = properties.length;
    handlePropertiesChange([...properties, newProp]);
    setExpandedProperties(new Set([...expandedProperties, newIndex]));
  };

  const removeProperty = (index: number) => {
    const newProperties = properties.filter((_, i) => i !== index);
    handlePropertiesChange(newProperties);

    // Update expanded indices: remove the deleted index and shift down higher indices
    const newExpanded = new Set<number>();
    expandedProperties.forEach((expandedIndex) => {
      if (expandedIndex < index) {
        newExpanded.add(expandedIndex);
      } else if (expandedIndex > index) {
        newExpanded.add(expandedIndex - 1);
      }
      // If expandedIndex === index, it's removed (not added to newExpanded)
    });
    setExpandedProperties(newExpanded);
  };

  const updateProperty = (index: number, updates: Partial<SchemaProperty>) => {
    const newProperties = [...properties];
    newProperties[index] = { ...newProperties[index], ...updates };
    handlePropertiesChange(newProperties);
  };

  const toggleExpanded = (index: number) => {
    const newExpanded = new Set(expandedProperties);
    if (newExpanded.has(index)) {
      newExpanded.delete(index);
    } else {
      newExpanded.add(index);
    }
    setExpandedProperties(newExpanded);
  };

  const handleRawJsonChange = (newJson: string) => {
    setRawJson(newJson);
    setRawJsonError("");

    try {
      const parsed = JSON.parse(newJson);
      if (typeof parsed !== "object" || Array.isArray(parsed)) {
        setRawJsonError("Schema must be a JSON object");
        return;
      }
      onChange(parsed);

      // Update properties from parsed JSON
      // Expects StackStorm-style flat format: { param_name: { type, required, secret, ... }, ... }
      const props: SchemaProperty[] = [];

      Object.entries(parsed).forEach(([name, propDef]) => {
        if (propDef && typeof propDef === "object" && !Array.isArray(propDef)) {
          const def = propDef as SchemaPropertyDef;
          props.push({
            name,
            type: def.type || "string",
            description: def.description || "",
            required: def.required === true,
            secret: def.secret === true,
            default:
              def.default !== undefined
                ? JSON.stringify(def.default)
                : undefined,
            minimum: def.minimum,
            maximum: def.maximum,
            minLength: def.minLength,
            maxLength: def.maxLength,
            pattern: def.pattern,
            enum: def.enum,
          });
        }
      });

      setProperties(props);
    } catch (e: unknown) {
      setRawJsonError(e instanceof Error ? e.message : "Invalid JSON");
    }
  };

  return (
    <div className={className}>
      {label && (
        <label className="block text-sm font-medium text-gray-700 mb-1">
          {label}
        </label>
      )}

      <div className="border border-gray-300 rounded-lg overflow-hidden">
        {/* Header with view toggle */}
        <div className="bg-gray-50 px-4 py-2 border-b border-gray-200 flex items-center justify-between">
          <span className="text-sm font-medium text-gray-700">
            {showRawJson ? "Raw JSON Schema" : "Schema Properties"}
            {disabled && (
              <span className="ml-2 text-xs px-2 py-0.5 bg-gray-200 text-gray-600 rounded">
                Read-only
              </span>
            )}
          </span>
          {!disabled && (
            <button
              type="button"
              onClick={() => {
                if (!showRawJson) {
                  setRawJson(JSON.stringify(buildSchema(), null, 2));
                  setRawJsonError("");
                }
                setShowRawJson(!showRawJson);
              }}
              className="text-sm text-blue-600 hover:text-blue-800 flex items-center gap-1"
            >
              <Code className="h-4 w-4" />
              {showRawJson ? "Visual Editor" : "Raw JSON"}
            </button>
          )}
        </div>

        {/* Content */}
        <div className="bg-white p-4">
          {showRawJson ? (
            // Raw JSON editor
            <div>
              <textarea
                value={rawJson}
                onChange={(e) => handleRawJsonChange(e.target.value)}
                rows={12}
                disabled={disabled}
                className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-xs ${
                  rawJsonError ? "border-red-500" : "border-gray-300"
                } ${disabled ? "bg-gray-100 cursor-not-allowed" : ""}`}
                placeholder={
                  placeholder || '{"type": "object", "properties": {...}}'
                }
              />
              {rawJsonError && (
                <p className="mt-1 text-sm text-red-600">{rawJsonError}</p>
              )}
            </div>
          ) : (
            // Visual property editor
            <div className="space-y-3">
              {properties.length === 0 ? (
                <div className="text-center py-8 text-gray-500">
                  <p className="text-sm">No properties defined</p>
                  <p className="text-xs mt-1">
                    Click "Add Property" to get started
                  </p>
                </div>
              ) : (
                properties.map((prop, index) => {
                  const isExpanded = expandedProperties.has(index);
                  return (
                    <div
                      key={index}
                      className="border border-gray-200 rounded-lg overflow-hidden"
                    >
                      {/* Property header */}
                      <div className="bg-gray-50 px-3 py-2 flex items-center justify-between">
                        <button
                          type="button"
                          onClick={() => toggleExpanded(index)}
                          className="flex items-center gap-2 flex-1 text-left"
                        >
                          {isExpanded ? (
                            <ChevronDown className="h-4 w-4 text-gray-500" />
                          ) : (
                            <ChevronRight className="h-4 w-4 text-gray-500" />
                          )}
                          <span className="font-mono text-sm font-medium text-gray-900">
                            {prop.name}
                          </span>
                          <span className="text-xs px-2 py-0.5 bg-blue-100 text-blue-700 rounded">
                            {prop.type}
                          </span>
                          {prop.required && (
                            <span className="text-xs px-2 py-0.5 bg-red-100 text-red-700 rounded">
                              Required
                            </span>
                          )}
                        </button>
                        {!disabled && (
                          <button
                            type="button"
                            onClick={() => removeProperty(index)}
                            className="text-red-600 hover:text-red-800 p-1"
                          >
                            <Trash2 className="h-4 w-4" />
                          </button>
                        )}
                      </div>

                      {/* Property details (collapsible) */}
                      {isExpanded && (
                        <div className="p-3 space-y-3 bg-white">
                          {/* Name */}
                          <div>
                            <label className="block text-xs font-medium text-gray-700 mb-1">
                              Property Name
                            </label>
                            <input
                              type="text"
                              value={prop.name}
                              onChange={(e) =>
                                updateProperty(index, { name: e.target.value })
                              }
                              disabled={disabled}
                              className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                                disabled ? "bg-gray-100 cursor-not-allowed" : ""
                              }`}
                            />
                          </div>

                          {/* Type */}
                          <div>
                            <label className="block text-xs font-medium text-gray-700 mb-1">
                              Type
                            </label>
                            <select
                              value={prop.type}
                              onChange={(e) =>
                                updateProperty(index, { type: e.target.value })
                              }
                              disabled={disabled}
                              className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                                disabled ? "bg-gray-100 cursor-not-allowed" : ""
                              }`}
                            >
                              {PROPERTY_TYPES.map((type) => (
                                <option key={type.value} value={type.value}>
                                  {type.label}
                                </option>
                              ))}
                            </select>
                          </div>

                          {/* Description */}
                          <div>
                            <label className="block text-xs font-medium text-gray-700 mb-1">
                              Description
                            </label>
                            <input
                              type="text"
                              value={prop.description}
                              onChange={(e) =>
                                updateProperty(index, {
                                  description: e.target.value,
                                })
                              }
                              placeholder="Describe this property..."
                              disabled={disabled}
                              className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                                disabled ? "bg-gray-100 cursor-not-allowed" : ""
                              }`}
                            />
                          </div>

                          {/* Required and Secret checkboxes */}
                          <div className="flex items-center gap-6">
                            <div className="flex items-center">
                              <input
                                type="checkbox"
                                id={`required-${index}`}
                                checked={prop.required}
                                onChange={(e) =>
                                  updateProperty(index, {
                                    required: e.target.checked,
                                  })
                                }
                                disabled={disabled}
                                className={`h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded ${
                                  disabled
                                    ? "cursor-not-allowed opacity-50"
                                    : ""
                                }`}
                              />
                              <label
                                htmlFor={`required-${index}`}
                                className="ml-2 text-xs font-medium text-gray-700"
                              >
                                Required
                              </label>
                            </div>
                            <div className="flex items-center">
                              <input
                                type="checkbox"
                                id={`secret-${index}`}
                                checked={prop.secret}
                                onChange={(e) =>
                                  updateProperty(index, {
                                    secret: e.target.checked,
                                  })
                                }
                                disabled={disabled}
                                className={`h-4 w-4 text-yellow-600 focus:ring-yellow-500 border-gray-300 rounded ${
                                  disabled
                                    ? "cursor-not-allowed opacity-50"
                                    : ""
                                }`}
                              />
                              <label
                                htmlFor={`secret-${index}`}
                                className="ml-2 text-xs font-medium text-gray-700"
                              >
                                Secret
                              </label>
                            </div>
                          </div>

                          {/* Default value */}
                          <div>
                            <label className="block text-xs font-medium text-gray-700 mb-1">
                              Default Value (optional)
                            </label>
                            <input
                              type="text"
                              value={prop.default || ""}
                              onChange={(e) =>
                                updateProperty(index, {
                                  default: e.target.value,
                                })
                              }
                              placeholder={
                                prop.type === "string"
                                  ? '"default value"'
                                  : prop.type === "number"
                                    ? "0"
                                    : prop.type === "boolean"
                                      ? "true"
                                      : ""
                              }
                              disabled={disabled}
                              className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono ${
                                disabled ? "bg-gray-100 cursor-not-allowed" : ""
                              }`}
                            />
                          </div>

                          {/* String-specific fields */}
                          {prop.type === "string" && (
                            <>
                              <div className="grid grid-cols-2 gap-2">
                                <div>
                                  <label className="block text-xs font-medium text-gray-700 mb-1">
                                    Min Length
                                  </label>
                                  <input
                                    type="number"
                                    value={prop.minLength || ""}
                                    onChange={(e) =>
                                      updateProperty(index, {
                                        minLength: e.target.value
                                          ? parseInt(e.target.value)
                                          : undefined,
                                      })
                                    }
                                    disabled={disabled}
                                    className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                                      disabled
                                        ? "bg-gray-100 cursor-not-allowed"
                                        : ""
                                    }`}
                                  />
                                </div>
                                <div>
                                  <label className="block text-xs font-medium text-gray-700 mb-1">
                                    Max Length
                                  </label>
                                  <input
                                    type="number"
                                    value={prop.maxLength || ""}
                                    onChange={(e) =>
                                      updateProperty(index, {
                                        maxLength: e.target.value
                                          ? parseInt(e.target.value)
                                          : undefined,
                                      })
                                    }
                                    disabled={disabled}
                                    className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                                      disabled
                                        ? "bg-gray-100 cursor-not-allowed"
                                        : ""
                                    }`}
                                  />
                                </div>
                              </div>
                              <div>
                                <label className="block text-xs font-medium text-gray-700 mb-1">
                                  Pattern (regex)
                                </label>
                                <input
                                  type="text"
                                  value={prop.pattern || ""}
                                  onChange={(e) =>
                                    updateProperty(index, {
                                      pattern: e.target.value,
                                    })
                                  }
                                  placeholder="^[a-z0-9_]+$"
                                  disabled={disabled}
                                  className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono ${
                                    disabled
                                      ? "bg-gray-100 cursor-not-allowed"
                                      : ""
                                  }`}
                                />
                              </div>
                            </>
                          )}

                          {/* Number-specific fields */}
                          {(prop.type === "number" ||
                            prop.type === "integer") && (
                            <div className="grid grid-cols-2 gap-2">
                              <div>
                                <label className="block text-xs font-medium text-gray-700 mb-1">
                                  Minimum
                                </label>
                                <input
                                  type="number"
                                  value={prop.minimum || ""}
                                  onChange={(e) =>
                                    updateProperty(index, {
                                      minimum: e.target.value
                                        ? parseFloat(e.target.value)
                                        : undefined,
                                    })
                                  }
                                  disabled={disabled}
                                  className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                                    disabled
                                      ? "bg-gray-100 cursor-not-allowed"
                                      : ""
                                  }`}
                                />
                              </div>
                              <div>
                                <label className="block text-xs font-medium text-gray-700 mb-1">
                                  Maximum
                                </label>
                                <input
                                  type="number"
                                  value={prop.maximum || ""}
                                  onChange={(e) =>
                                    updateProperty(index, {
                                      maximum: e.target.value
                                        ? parseFloat(e.target.value)
                                        : undefined,
                                    })
                                  }
                                  disabled={disabled}
                                  className={`w-full px-2 py-1 text-sm border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                                    disabled
                                      ? "bg-gray-100 cursor-not-allowed"
                                      : ""
                                  }`}
                                />
                              </div>
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })
              )}

              {/* Add property button */}
              {!disabled && (
                <button
                  type="button"
                  onClick={addProperty}
                  className="w-full px-4 py-2 border-2 border-dashed border-gray-300 rounded-lg text-gray-600 hover:border-blue-500 hover:text-blue-600 flex items-center justify-center gap-2 transition-colors"
                >
                  <Plus className="h-4 w-4" />
                  Add Property
                </button>
              )}
            </div>
          )}
        </div>
      </div>

      {error && <p className="mt-1 text-sm text-red-600">{error}</p>}
    </div>
  );
}
