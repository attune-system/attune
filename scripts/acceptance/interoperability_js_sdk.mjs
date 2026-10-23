// Real JS SDK calls, not a substituted fetch implementation. Pass the installed
// SDK entrypoint as argv[2]; credentials and operation arrive only on stdin.
import { pathToFileURL } from "node:url";

const unwrap = (value) => value?.data ?? value;
try {
  let input = "";
  for await (const chunk of process.stdin) {
    input += chunk;
    if (input.length > 1024 * 1024) throw new Error("Input limit");
  }
  const r = JSON.parse(input);
  const { AttuneClient, ExternalApprovals, AttuneApiError } = await import(pathToFileURL(process.argv[2]).href);
  const client = new AttuneClient({ apiUrl: r.api_url, apiToken: r.access_token });
  const approvals = new ExternalApprovals({ apiUrl: r.api_url, accessToken: r.access_token });
  const pack = `/api/v1/packs/${encodeURIComponent(r.pack ?? "")}`;
  const reference = encodeURIComponent(r.ref ?? "");
  let result;
  if (r.expect_denial) {
    assertExecute(r.operation);
    try {
      await client.post("/api/v1/executions/execute", { json: r.body });
    } catch (error) {
      if (!(error instanceof AttuneApiError)) throw error;
      result = { native_denial: { status: error.status, code: error.code, request_id: error.requestId } };
    }
    if (result === undefined) throw new Error("Native SDK accepted revoked/expired authority");
  }
  if (!r.expect_denial) switch (r.operation) {
    case "enqueue": result = await client.post(`/api/v1/queues/${reference}/items`, { json: r.body }); break;
    case "runtime_issue": result = await approvals.issueRuntime(r.pack, r.body); break;
    case "runtime_replace": result = await approvals.replaceRuntime(r.pack, r.id, r.body); break;
    case "targets": result = await approvals.listInputApprovalTargets({ packRef: r.pack }); break;
    case "issue": result = await approvals.issueInput(r.pack, r.body); break;
    case "approval_get": result = await approvals.get(r.pack, r.id); break;
    case "approval_revoke": result = await approvals.revoke(r.pack, r.id); break;
    case "execute": result = await client.post("/api/v1/executions/execute", { json: r.body }); break;
    case "pack_get": result = await client.get(pack); break;
    case "config_set": result = await client.put(pack, { json: { config: r.config } }); break;
    case "config_reset": result = await client.put(pack, { json: { config_reset_fields: r.fields } }); break;
    case "schema_get": result = await client.get(`/api/v1/schema-types/${reference}`); break;
    case "queue_get": result = await client.get(`/api/v1/queues/${reference}`); break;
    case "queue_grouping": {
      const path = `/api/v1/queues/${reference}`;
      const record = unwrap(await client.get(path));
      const config = { ...record.config, dispatch: { ...record.config?.dispatch, separate_requesters: r.grouping === "separate" } };
      result = await client.put(path, { json: { config } });
      break;
    }
    case "permission_export": {
      const record = unwrap(await client.get(`/api/v1/permissions/sets/by-ref/${reference}`));
      if (!record.authored_definition) throw new Error("Canonical authoring definition missing");
      result = { ...record.authored_definition, label: record.label, description: record.description };
      break;
    }
    case "permission_import": {
      const record = unwrap(await client.get(`/api/v1/permissions/sets/by-ref/${reference}`));
      const { ref: ignored, ...body } = r.definition;
      result = await client.put(`/api/v1/permissions/sets/${record.id}`, { json: body });
      break;
    }
    default: throw new Error("Unsupported SDK operation");
  }
  process.stdout.write(JSON.stringify(result));
} catch {
  process.stderr.write("Real JS SDK operation failed; response and credentials suppressed\n");
  process.exitCode = 1;
}

function assertExecute(operation) {
  if (operation !== "execute") throw new Error("Denial probe must execute the actual invocation");
}
