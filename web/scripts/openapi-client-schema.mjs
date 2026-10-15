// openapi-typescript-codegen expects nullable rather than OpenAPI 3.1 type arrays.
// Keep the published spec unchanged; adapt only the generator's input.
export function clientSchema(document) {
  function visit(value) {
    if (Array.isArray(value)) return value.map(visit);
    if (value === null || typeof value !== "object") return value;
    const result = Object.fromEntries(
      Object.entries(value).map(([key, child]) => [
        key,
        ["example", "examples", "default", "enum"].includes(key)
          ? child
          : visit(child),
      ]),
    );
    if (Array.isArray(result.type)) {
      const types = result.type.filter((type) => type !== "null");
      const nullable = result.type.includes("null");
      delete result.type;
      if (types.length === 1) result.type = types[0];
      else result.oneOf = types.map((type) => ({ type }));
      if (nullable) result.nullable = true;
    }
    return result;
  }
  const result = visit(document);
  // serde_json::Value is unconstrained JSON, including scalars and null.
  if (result.components?.schemas?.Value) {
    result.components.schemas.Value = {
      oneOf: [
        { type: "string", nullable: true },
        { type: "number" },
        { type: "boolean" },
        { type: "array", items: { $ref: "#/components/schemas/Value" } },
        {
          type: "object",
          additionalProperties: { $ref: "#/components/schemas/Value" },
        },
      ],
    };
  }
  return result;
}

// Inline discriminant enums share a generated namespace and lose variants.
// Generate these models with literal unions, retaining enums elsewhere.
export function discriminatedModels(document) {
  return Object.entries(document.components.schemas)
    .filter(([, schema]) =>
      schema.oneOf?.some((branch) => branch.properties?.op?.enum),
    )
    .map(([name]) => name);
}
