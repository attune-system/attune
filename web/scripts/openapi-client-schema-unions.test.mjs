import assert from "node:assert/strict";
import test from "node:test";
import { clientSchema, discriminatedModels } from "./openapi-client-schema.mjs";

const document = (schemas) => ({ components: { schemas } });
const ref = (name) => ({ $ref: `#/components/schemas/${name}` });
const branch = (tag, literal) => ({
  type: "object",
  required: [tag],
  properties: { [tag]: { type: "string", enum: [literal] } },
});

test("local references never follow inherited properties, but allow own prototype-like names", () => {
  for (const name of ["constructor", "__proto__", "prototype"]) {
    const schemas = { Model: { oneOf: [branch("kind", "a"), ref(name)] } };
    Object.setPrototypeOf(schemas, { [name]: branch("kind", "b") });
    assert.deepEqual(discriminatedModels(document(schemas)), []);
    Object.defineProperty(schemas, name, {
      value: branch("kind", "b"),
      enumerable: true,
    });
    assert.deepEqual(discriminatedModels(document(schemas)), ["Model"]);
  }
});

// Retained wire shapes from openapi-public-input-binary.json. These models
// intentionally have no explicit OpenAPI discriminator.
const approvalPurpose = {
  oneOf: [
    branch("kind", "runtime"),
    {
      type: "object",
      required: ["targets", "kind"],
      properties: {
        kind: { type: "string", enum: ["declared_input"] },
        targets: {
          type: "array",
          items: ref("DeclaredInputApprovalTarget"),
          description:
            "Only currently visible retained targets. Empty when target rows were\ncollected or current component visibility denies disclosure. Purpose\nnever falls back to runtime merely because no target is visible.",
          maxItems: 256,
        },
      },
    },
  ],
};
const invocationBinding = {
  oneOf: [
    {
      type: "object",
      required: ["action_ref", "kind"],
      properties: {
        action_ref: { type: "string" },
        kind: { type: "string", enum: ["action"] },
      },
    },
    {
      type: "object",
      required: ["workflow_ref", "task_path", "action_ref", "kind"],
      properties: {
        action_ref: { type: "string" },
        kind: { type: "string", enum: ["workflow_task"] },
        task_path: { type: "string" },
        workflow_ref: { type: "string" },
      },
    },
    {
      type: "object",
      required: ["rule_ref", "action_ref", "kind"],
      properties: {
        action_ref: { type: "string" },
        kind: { type: "string", enum: ["rule"] },
        rule_ref: { type: "string" },
      },
    },
    {
      type: "object",
      required: ["queue_ref", "action_ref", "kind"],
      properties: {
        action_ref: { type: "string" },
        kind: { type: "string", enum: ["queue"] },
        queue_ref: { type: "string" },
      },
    },
  ],
};

test("actual approval and invocation unions retain every canonical kind literal", () => {
  const source = document({
    ApprovalPurpose: approvalPurpose,
    InvocationBinding: invocationBinding,
  });
  const original = structuredClone(source);
  const adapted = clientSchema(source);
  assert.deepEqual(discriminatedModels(adapted), [
    "ApprovalPurpose",
    "InvocationBinding",
  ]);
  assert.deepEqual(adapted, original);
  assert.deepEqual(source, original);
  assert.deepEqual(
    adapted.components.schemas.ApprovalPurpose.oneOf.map(
      (b) => b.properties.kind.enum,
    ),
    [["runtime"], ["declared_input"]],
  );
  assert.deepEqual(
    adapted.components.schemas.InvocationBinding.oneOf.map(
      (b) => b.properties.kind.enum,
    ),
    [["action"], ["workflow_task"], ["rule"], ["queue"]],
  );
});

test("existing op/type unions and anyOf use structure, not model names", () => {
  const source = document({
    ArtifactStringPatch: {
      oneOf: [branch("op", "set"), branch("op", "clear")],
    },
    InstallSource: {
      oneOf: [branch("type", "git"), branch("type", "archive")],
    },
    ArbitrarilyNamed: {
      anyOf: [branch("status", ""), branch("status", "ready")],
    },
  });
  assert.deepEqual(discriminatedModels(source), [
    "ArbitrarilyNamed",
    "ArtifactStringPatch",
    "InstallSource",
  ]);
});

test("local referenced branches, tag schemas and union aliases are resolved", () => {
  const source = document({
    ZUnion: { anyOf: [ref("First"), ref("SecondAlias")] },
    AUnionAlias: ref("ZUnion"),
    First: branch("kind", "first"),
    SecondAlias: ref("Second"),
    Second: {
      type: "object",
      required: ["kind"],
      properties: { kind: ref("tag~1second~0") },
    },
    "tag/second~": { const: "second" },
  });
  const original = structuredClone(source);
  assert.deepEqual(discriminatedModels(source), ["AUnionAlias", "ZUnion"]);
  const adapted = clientSchema(source);
  assert.deepEqual(adapted.components.schemas["tag/second~"], {
    const: "second",
    type: "string",
    enum: ["second"],
  });
  assert.deepEqual(discriminatedModels(adapted), ["AUnionAlias", "ZUnion"]);
  assert.deepEqual(source, original);
});

test("const tags get equivalent generator enums without changing canonical literals", () => {
  const source = document({
    Model: {
      oneOf: [
        {
          type: "object",
          required: ["tag"],
          properties: { tag: { type: "string", const: "a" } },
        },
        branch("tag", "b"),
      ],
    },
  });
  assert.deepEqual(discriminatedModels(source), ["Model"]);
  const adapted = clientSchema(source);
  assert.deepEqual(adapted.components.schemas.Model.oneOf[0].properties.tag, {
    type: "string",
    const: "a",
    enum: ["a"],
  });
  assert.equal(
    source.components.schemas.Model.oneOf[0].properties.tag.enum,
    undefined,
  );
  assert.deepEqual(discriminatedModels(adapted), ["Model"]);
});

const invalidBranches = {
  overlapping: branch("kind", "a"),
  optional: { ...branch("kind", "b"), required: [] },
  missing: { type: "object", required: ["kind"], properties: {} },
  multiEnum: {
    type: "object",
    required: ["kind"],
    properties: { kind: { type: "string", enum: ["b", "c"] } },
  },
  nonString: {
    type: "object",
    required: ["kind"],
    properties: { kind: { const: 2 } },
  },
  contradictory: {
    type: "object",
    required: ["kind"],
    properties: { kind: { const: "b", enum: ["c"] } },
  },
  nullable: { ...branch("kind", "b"), nullable: true },
  nullableTag: {
    type: "object",
    required: ["kind"],
    properties: { kind: { type: ["string", "null"], enum: ["b"] } },
  },
  composed: { allOf: [branch("kind", "b")] },
  external: { $ref: "https://example.invalid/schema" },
  missingRef: ref("Missing"),
  refCycle: ref("CycleA"),
  constrainedRef: { ...ref("Other"), required: ["different"] },
};
for (const [name, invalid] of Object.entries(invalidBranches)) {
  test(`does not infer a tag for ${name} branches`, () => {
    const source = document({
      Model: { oneOf: [branch("kind", "a"), invalid] },
      CycleA: ref("CycleB"),
      CycleB: ref("CycleA"),
      Other: branch("kind", "b"),
    });
    assert.deepEqual(discriminatedModels(source), []);
    assert.deepEqual(discriminatedModels(clientSchema(source)), []);
  });
}

test("requires a common tag across every branch, even with explicit discriminator metadata", () => {
  const source = document({
    DifferentTags: { oneOf: [branch("kind", "a"), branch("type", "b")] },
    Untagged: {
      discriminator: { propertyName: "kind" },
      oneOf: [{ type: "string" }, { type: "number" }],
    },
    Single: { oneOf: [branch("kind", "a")] },
    Mixed: {
      oneOf: [branch("kind", "a"), branch("kind", "b"), { type: "null" }],
    },
    AmbiguousComposition: {
      oneOf: [branch("kind", "a"), branch("kind", "b")],
      anyOf: [],
    },
  });
  assert.deepEqual(discriminatedModels(source), []);
  assert.deepEqual(discriminatedModels({}), []);
});

test("recursive Value remains JSON and schema discovery does not recurse into payloads", () => {
  const source = document({
    Value: {},
    Model: {
      oneOf: [
        branch("kind", "a"),
        {
          ...branch("kind", "b"),
          properties: {
            ...branch("kind", "b").properties,
            payload: ref("Value"),
          },
        },
      ],
    },
  });
  const adapted = clientSchema(source);
  assert.deepEqual(discriminatedModels(adapted), ["Model"]);
  assert.equal(adapted.components.schemas.Value.oneOf.length, 5);
  assert.deepEqual(
    adapted.components.schemas.Value.oneOf[3].items,
    ref("Value"),
  );
  assert.deepEqual(
    adapted.components.schemas.Value.oneOf[4].additionalProperties,
    ref("Value"),
  );
});
