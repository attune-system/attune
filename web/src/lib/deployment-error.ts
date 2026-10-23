function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function deploymentDiagnosticsText(details: unknown): string {
  if (!object(details)) return "";
  const lines: string[] = [];
  if (typeof details.stage === "string")
    lines.push(`Stage: ${details.stage.replaceAll("_", " ")}`);
  if (typeof details.outcome === "string")
    lines.push(`Outcome: ${details.outcome.replaceAll("_", " ")}`);
  if (Array.isArray(details.diagnostics)) {
    for (const diagnostic of details.diagnostics) {
      if (!object(diagnostic) || typeof diagnostic.message !== "string")
        continue;
      const file = typeof diagnostic.path === "string" ? diagnostic.path : "";
      const path =
        typeof diagnostic.metadata_path === "string"
          ? diagnostic.metadata_path
          : "";
      const line =
        typeof diagnostic.line === "number" ? ` line ${diagnostic.line}` : "";
      lines.push(
        `${file}${path ? ` at ${path}` : ""}${line}: ${diagnostic.message}`,
      );
      if (typeof diagnostic.hint === "string") lines.push(diagnostic.hint);
    }
  }
  if (typeof details.install_id === "number")
    lines.push(
      `Install ${details.install_id}. Check install status before retrying.`,
    );
  return lines.join("\n");
}

export function deploymentErrorText(error: unknown, fallback: string): string {
  if (!object(error)) return fallback;
  let body: unknown = "body" in error ? error.body : undefined;
  if (!object(body) && object(error.response) && object(error.response.data))
    body = error.response.data;
  if (!object(body)) return fallback;
  const message =
    typeof body.error === "string"
      ? body.error
      : typeof body.message === "string"
        ? body.message
        : fallback;
  const details = deploymentDiagnosticsText(body.details);
  const code = typeof body.code === "string" ? `\nCode: ${body.code}` : "";
  return `${message}${code}${details ? `\n${details}` : ""}`;
}
