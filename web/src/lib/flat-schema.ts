/** A single property definition within a flat schema object. */
export interface SchemaPropertyDef {
  type?: string | string[];
  description?: string;
  required?: boolean | string[];
  secret?: boolean;
  default?: unknown;
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  enum?: unknown[];
  [key: string]: unknown;
}

/** Each parameter name maps to its definition. */
export type FlatSchema = Record<string, SchemaPropertyDef>;

export function isStringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) && value.every((item) => typeof item === "string")
  );
}

function isDefinition(value: unknown): value is SchemaPropertyDef {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return false;
  for (const [name, field] of Object.entries(value)) {
    if (["type", "required"].includes(name)) {
      if (name === "type" && typeof field === "string") continue;
      if (name === "required" && typeof field === "boolean") continue;
      if (!isStringArray(field)) return false;
    }
    if (["description", "pattern"].includes(name) && typeof field !== "string")
      return false;
    if (name === "secret" && typeof field !== "boolean") return false;
    if (
      ["minimum", "maximum", "minLength", "maxLength"].includes(name) &&
      typeof field !== "number"
    )
      return false;
    if (name === "enum" && !Array.isArray(field)) return false;
  }
  return true;
}

export function isFlatSchema(value: unknown): value is FlatSchema {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Object.values(value).every(isDefinition)
  );
}
