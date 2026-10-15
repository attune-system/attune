import { execFileSync } from "node:child_process";
import {
  cpSync,
  mkdtempSync,
  rmSync,
  readFileSync,
  writeFileSync,
  existsSync,
  readdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { clientSchema, discriminatedModels } from "./openapi-client-schema.mjs";

const webDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const workspaceDir = resolve(webDir, "..");
const outputDir = join(webDir, "src", "api");
const checkedInSpecPath = join(webDir, "openapi.json");
const inputIndex = process.argv.indexOf("--input");
const inputPath = inputIndex === -1 ? undefined : process.argv[inputIndex + 1];
const check = process.argv.includes("--check");
const outputIndex = process.argv.indexOf("--output");
if (
  outputIndex !== -1 &&
  (!process.argv[outputIndex + 1] ||
    process.argv[outputIndex + 1].startsWith("--"))
) {
  throw new Error("--output requires a directory path");
}
const outputPath =
  outputIndex === -1
    ? outputDir
    : resolve(webDir, process.argv[outputIndex + 1]);
const modelsIndex = process.argv.indexOf("--models");
const selectedModels =
  modelsIndex === -1 ? undefined : process.argv[modelsIndex + 1]?.split(",");
if (
  modelsIndex !== -1 &&
  (!selectedModels?.length ||
    selectedModels.some((name) => !/^[A-Za-z0-9_]+$/.test(name)))
) {
  throw new Error(
    "--models requires comma-separated model names without paths",
  );
}
if (inputIndex !== -1 && (!inputPath || inputPath.startsWith("--"))) {
  throw new Error("--input requires an OpenAPI spec path");
}
const temporaryDir = mkdtempSync(join(tmpdir(), "attune-web-api-"));
const specPath = inputPath
  ? resolve(webDir, inputPath)
  : join(temporaryDir, "openapi.json");
const generatedDir = join(temporaryDir, "generated");

try {
  if (!inputPath) {
    execFileSync(
      "cargo",
      [
        "run",
        "-q",
        "-p",
        "attune-api",
        "--bin",
        "export-openapi",
        "--",
        specPath,
      ],
      { cwd: workspaceDir, stdio: "inherit" },
    );
    if (!check) cpSync(specPath, checkedInSpecPath);
  }

  const document = JSON.parse(readFileSync(specPath, "utf8"));
  const generatorSpecPath = join(temporaryDir, "client-openapi.json");
  writeFileSync(generatorSpecPath, JSON.stringify(clientSchema(document)));

  execFileSync(
    join(webDir, "node_modules", ".bin", "openapi"),
    [
      "--input",
      generatorSpecPath,
      "--output",
      generatedDir,
      "--client",
      "axios",
      "--useOptions",
    ],
    { cwd: webDir, stdio: "inherit" },
  );

  const unionModels = discriminatedModels(document);
  if (unionModels.length > 0) {
    const unionDir = join(temporaryDir, "unions");
    execFileSync(
      join(webDir, "node_modules", ".bin", "openapi"),
      [
        "--input",
        generatorSpecPath,
        "--output",
        unionDir,
        "--client",
        "axios",
        "--useOptions",
        "--useUnionTypes",
      ],
      { cwd: webDir, stdio: "inherit" },
    );
    let index = readFileSync(join(generatedDir, "index.ts"), "utf8");
    for (const model of unionModels) {
      cpSync(
        join(unionDir, "models", `${model}.ts`),
        join(generatedDir, "models", `${model}.ts`),
      );
      index = index.replace(`export { ${model} }`, `export type { ${model} }`);
    }
    writeFileSync(join(generatedDir, "index.ts"), index);
  }

  // Recursive aliases must use an index signature, not the Record mapped type.
  const valuePath = join(generatedDir, "models", "Value.ts");
  if (existsSync(valuePath)) {
    writeFileSync(
      valuePath,
      readFileSync(valuePath, "utf8").replace(
        /Record<string,\s*Value>/g,
        "{ [key: string]: Value }",
      ),
    );
  }

  // Preserve safe user-facing error rendering across client regeneration.
  const requestPath = join(generatedDir, "core", "request.ts");
  let request = readFileSync(requestPath, "utf8");
  request = request.replace(
    /(import \{ ApiError \} from ['"]\.\/ApiError['"];)/,
    '$1\nimport { deploymentErrorText } from "@/lib/deployment-error";',
  );
  request = request.replace(
    "throw new ApiError(options, result, error);",
    "throw new ApiError(options, result, deploymentErrorText({ body: result.body }, error));",
  );
  request = request.replace(
    /const errorBody = \(\(\) => \{[\s\S]*?\}\)\(\);/,
    "",
  );
  request = request.replace(
    /`Generic Error:[^`]*`/,
    "deploymentErrorText({ body: result.body }, `Request failed: ${errorStatus} ${errorStatusText}. Check API availability and install status before retrying.`)",
  );
  writeFileSync(requestPath, request);

  execFileSync(
    join(webDir, "node_modules", ".bin", "prettier"),
    ["--config", join(webDir, ".prettierrc.json"), "--write", generatedDir],
    { cwd: webDir, stdio: "pipe" },
  );

  if (check) {
    const differences = [];
    function compare(relativePath) {
      const generated = join(generatedDir, relativePath);
      const existing = join(outputPath, relativePath);
      if (
        !existsSync(existing) ||
        readFileSync(generated, "utf8") !== readFileSync(existing, "utf8")
      )
        differences.push(relativePath);
    }
    if (selectedModels) {
      for (const model of selectedModels) compare(`models/${model}.ts`);
    } else {
      compare("index.ts");
      for (const directory of ["core", "models", "services"]) {
        for (const file of readdirSync(join(generatedDir, directory)))
          compare(`${directory}/${file}`);
        for (const file of readdirSync(join(outputPath, directory))) {
          if (!existsSync(join(generatedDir, directory, file)))
            differences.push(`${directory}/${file} (removed)`);
        }
      }
    }
    if (differences.length > 0) {
      throw new Error(
        `Generated API client is out of date:\n${differences.join("\n")}\nRun npm run generate:api.`,
      );
    }
    console.log("Generated API client is current.");
  } else if (selectedModels) {
    // A scoped regeneration preserves unrelated working-tree client changes.
    for (const model of selectedModels) {
      cpSync(
        join(generatedDir, "models", `${model}.ts`),
        join(outputPath, "models", `${model}.ts`),
      );
    }
  } else {
    for (const directory of ["core", "models", "services"]) {
      rmSync(join(outputPath, directory), { recursive: true, force: true });
      cpSync(join(generatedDir, directory), join(outputPath, directory), {
        recursive: true,
      });
    }
    cpSync(join(generatedDir, "index.ts"), join(outputPath, "index.ts"));
  }
} finally {
  rmSync(temporaryDir, { recursive: true, force: true });
}
