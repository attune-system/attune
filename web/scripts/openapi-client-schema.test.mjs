import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import ts from "typescript";
import { ESLint } from "eslint";
import { clientSchema, discriminatedModels } from "./openapi-client-schema.mjs";

const webDir = dirname(dirname(fileURLToPath(import.meta.url)));

test("lint rejects request-body and request-field assertions", async () => {
  const linter = new ESLint({ cwd: webDir });
  for (const source of [
    "export const request = {} as unknown as UpdateRuleRequest;",
    'export const patch = {} as unknown as UpdateActionRequest["timeout_seconds"];',
  ]) {
    const [result] = await linter.lintText(source, {
      filePath: join(webDir, "src/request-cast-contract.ts"),
    });
    assert.ok(
      result.messages.some(
        (message) => message.ruleId === "no-restricted-syntax",
      ),
    );
  }
});
const document = {
  openapi: "3.1.0",
  info: { title: "Contract fixture", version: "1" },
  paths: {},
  components: {
    schemas: {
      Value: {},
      UpdateRequest: {
        type: "object",
        additionalProperties: false,
        properties: {
          refs: { type: ["array", "null"], items: { type: "string" } },
        },
      },
      StringPatch: {
        oneOf: [
          {
            type: "object",
            required: ["op", "value"],
            properties: {
              op: { type: "string", enum: ["set"] },
              value: { type: "string" },
            },
          },
          {
            type: "object",
            required: ["op"],
            properties: { op: { type: "string", enum: ["clear"] } },
          },
        ],
      },
    },
  },
};

test("normalizes nullable schemas without changing the published spec or examples", () => {
  const source = structuredClone(document);
  source.components.schemas.UpdateRequest.example = {
    type: ["string", "null"],
  };
  const before = structuredClone(source);
  const normalized = clientSchema(source);
  assert.deepEqual(source, before);
  assert.deepEqual(
    normalized.components.schemas.UpdateRequest.properties.refs,
    {
      type: "array",
      nullable: true,
      items: { type: "string" },
    },
  );
  assert.deepEqual(normalized.components.schemas.UpdateRequest.example, {
    type: ["string", "null"],
  });
  assert.deepEqual(discriminatedModels(source), ["StringPatch"]);
});

test("generation preserves discriminants and typed arrays; drift checks are read-only", () => {
  const root = mkdtempSync(join(tmpdir(), "attune-client-contract-"));
  try {
    const input = join(root, "openapi.json");
    const output = join(root, "client");
    writeFileSync(input, JSON.stringify(document));
    const args = [
      join(webDir, "scripts/generate-api.mjs"),
      "--input",
      input,
      "--output",
      output,
    ];
    execFileSync(process.execPath, args);
    const patch = readFileSync(join(output, "models/StringPatch.ts"), "utf8");
    assert.match(patch, /op: "set";\s+value: string;/);
    assert.match(patch, /op: "clear";/);
    assert.match(
      readFileSync(join(output, "models/UpdateRequest.ts"), "utf8"),
      /refs\?: Array<string> \| null/,
    );
    assert.match(
      readFileSync(join(output, "models/Value.ts"), "utf8"),
      /string \| null \| number \| boolean/,
    );
    const program = ts.createProgram(
      ["Value", "StringPatch", "UpdateRequest"].map((name) =>
        join(output, "models", `${name}.ts`),
      ),
      {
        strict: true,
        noEmit: true,
        skipLibCheck: true,
        types: [],
        target: ts.ScriptTarget.ES2022,
      },
    );
    assert.deepEqual(
      ts
        .getPreEmitDiagnostics(program)
        .map((diagnostic) =>
          ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"),
        ),
      [],
    );
    assert.match(
      readFileSync(join(output, "index.ts"), "utf8"),
      /export type \{ StringPatch \}/,
    );
    const request = readFileSync(join(output, "core/request.ts"), "utf8");
    assert.match(request, /deploymentErrorText/);
    assert.doesNotMatch(request, /JSON.stringify\(result.body/);
    execFileSync(process.execPath, [...args, "--check"]);
    const modelPath = join(output, "models/StringPatch.ts");
    writeFileSync(modelPath, patch + "// drift\n");
    assert.throws(
      () =>
        execFileSync(process.execPath, [...args, "--check"], { stdio: "pipe" }),
      /out of date/,
    );
    assert.equal(readFileSync(modelPath, "utf8"), patch + "// drift\n");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
