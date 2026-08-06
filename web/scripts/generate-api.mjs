import { execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const webDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const workspaceDir = resolve(webDir, "..");
const outputDir = join(webDir, "src", "api");
const checkedInSpecPath = join(webDir, "openapi.json");
const temporaryDir = mkdtempSync(join(tmpdir(), "attune-web-api-"));
const specPath = join(temporaryDir, "openapi.json");
const generatedDir = join(temporaryDir, "generated");

try {
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
  cpSync(specPath, checkedInSpecPath);

  execFileSync(
    join(webDir, "node_modules", ".bin", "openapi"),
    [
      "--input",
      specPath,
      "--output",
      generatedDir,
      "--client",
      "axios",
      "--useOptions",
    ],
    { cwd: webDir, stdio: "inherit" },
  );

  execFileSync(
    join(webDir, "node_modules", ".bin", "prettier"),
    ["--config", join(webDir, ".prettierrc.json"), "--write", generatedDir],
    { cwd: webDir, stdio: "inherit" },
  );

  for (const directory of ["core", "models", "services"]) {
    rmSync(join(outputDir, directory), { recursive: true, force: true });
    cpSync(join(generatedDir, directory), join(outputDir, directory), {
      recursive: true,
    });
  }
  cpSync(join(generatedDir, "index.ts"), join(outputDir, "index.ts"));
} finally {
  rmSync(temporaryDir, { recursive: true, force: true });
}
