// Central-only. Uses an existing Playwright module and owned Chromium executable.
// No request routing, fixtures, saved auth state, screenshots, HARs or traces.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";

async function main(r) {
  for (const origin of [r.web_url, r.api_url]) {
    const url = new URL(origin);
    assert.equal(url.protocol, "http:");
    assert.equal(url.hostname, "127.0.0.1");
    assert.ok(url.port);
    assert.equal(url.username, "");
  }
  const { chromium } = await import(pathToFileURL(process.argv[2]).href);
  const browser = await chromium.launch({ executablePath: r.browser_executable, headless: true });
  process.once("SIGTERM", () => {
    void browser.close().finally(() => process.exit(143));
  });
  let context;
  let page;
  try {
    context = await browser.newContext({ serviceWorkers: "block" });
    page = await context.newPage();
  } catch (failure) {
    await browser.close();
    throw failure;
  }
  page.setDefaultTimeout(30_000);
  const consoleErrors = [];
  let secretLeak = false;
  let responseReadFailure = false;
  const responseChecks = [];
  function checkText(text) {
    if (Object.values(r.credentials).some((credential) => text.includes(credential.password))) secretLeak = true;
    // Provisioner's secret is a random token_urlsafe(32). Compare candidates
    // using only its digest; never send its value into this browser harness.
    for (const match of text.matchAll(/[A-Za-z0-9_-]{43,4096}/g)) {
      for (let index = 0; index <= match[0].length - 43; index++) {
        const candidate = match[0].slice(index, index + 43);
        if (createHash("sha256").update(JSON.stringify(candidate)).digest("hex") === r.forbidden_source_sha256) secretLeak = true;
      }
    }
  }
  page.on("console", (message) => checkText(message.text()));
  page.on("response", (response) => {
    if ((response.headers()["content-type"] ?? "").includes("application/json")) {
      responseChecks.push(response.text().then((text) => {
        assert.ok(text.length <= 4 * 1024 * 1024);
        checkText(text);
      }).catch(() => { responseReadFailure = true; }));
    }
  });
  async function confidentiality() {
    checkText(await page.locator("body").innerText());
    let checked = 0;
    while (checked < responseChecks.length) {
      assert.ok(responseChecks.length <= 512);
      const current = responseChecks.slice(checked);
      checked = responseChecks.length;
      await Promise.all(current);
    }
    assert.equal(responseReadFailure, false);
    assert.equal(secretLeak, false);
    assert.equal(consoleErrors.length, 0);
  }
  // Keep only a count. Neither console content nor token-bearing requests are
  // written to evidence, even on assertion failure.
  page.on("pageerror", () => consoleErrors.push(true));
  try {
    const credential = r.credentials[r.phase === "consent" ? "sponsor" : "runtimeconsumer"];
    await page.goto(r.web_url + "/login");
    await page.locator("#login").fill(credential.login);
    await page.locator("#password").fill(credential.password);
    const login = page.waitForResponse((response) => new URL(response.url()).pathname === "/auth/login" && response.request().method() === "POST");
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    assert.equal((await login).status(), 200);
    await page.waitForURL((url) => url.pathname !== "/login");
    if (r.phase === "consent") {
      await page.goto(r.web_url + `/packs/${encodeURIComponent(r.pack)}/approvals`);
      const discovery = page.waitForResponse((response) => new URL(response.url()).pathname === `/api/v1/packs/${r.pack}/input-approval-targets`);
      await page.getByRole("button", { name: "New input-only consent", exact: true }).click();
      const targets = await (await discovery).json();
      const index = targets.items.findIndex((target) => ["policy_revision", "invocation_binding", "declaration_revision", "source_path", "expression"].every((key) => isDeepStrictEqual(target[key], r.body[key])));
      assert.ok(index >= 0);
      await page.getByRole("button", { name: `Select input target ${index + 1}`, exact: true }).click();
      await page.getByLabel("Fixed grants and optional expiry").fill(JSON.stringify({ grants: r.body.grants }));
      const issued = page.waitForResponse((response) => new URL(response.url()).pathname === `/api/v1/packs/${r.pack}/external-input-approvals` && response.request().method() === "POST");
      await page.getByRole("button", { name: "Issue input-only consent", exact: true }).click();
      const response = await issued;
      assert.ok([200, 201].includes(response.status()));
      assert.deepEqual(response.request().postDataJSON(), r.body);
      const approval = (await response.json()).data;
      assert.equal(approval.purpose.kind, "declared_input");
      assert.equal(approval.purpose.targets.length, 1);
      const heading = page.getByRole("heading", { name: `Approval ${approval.id}: active`, exact: true });
      await heading.waitFor();
      const row = page.getByRole("listitem").filter({ has: heading });
      await row.getByText("Purpose: Input-only consent", { exact: true }).waitFor();
      await row.getByRole("button", { name: "Revoke", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Confirm approval revocation" });
      await dialog.getByText(/input-only consent, not a runtime permission selection/).waitFor();
      const revoked = page.waitForResponse((result) => new URL(result.url()).pathname === `/api/v1/packs/${r.pack}/external-approvals/${approval.id}/revoke` && result.request().method() === "POST");
      await dialog.getByRole("button", { name: "Confirm revocation", exact: true }).click();
      assert.equal((await revoked).status(), 200);
      await page.getByRole("heading", { name: `Approval ${approval.id}: revoked`, exact: true }).waitFor();
      await confidentiality();
      return { real_browser: true, approval_id: approval.id, purpose: "declared_input", revoked: true };
    }
    assert.ok(["execute", "rerun"].includes(r.phase));
    const executions = [];
    for (const mode of r.phase === "rerun" ? ["default", "old_explicit_empty", "old_explicit_refs"] : ["default", "empty", "explicit"]) {
      const rerun = mode.startsWith("old_explicit_");
      const custom = mode === "explicit" || mode === "old_explicit_refs";
      const oldExecution = mode === "old_explicit_refs" ? r.old_explicit_execution_id : r.old_execution_id;
      await page.goto(r.web_url + (rerun ? `/executions/${oldExecution}` : `/actions/${encodeURIComponent(r.action)}`));
      const optionsResponse = page.waitForResponse((response) => new URL(response.url()).pathname === `/api/v1/actions/${r.action}/execution-options`);
      await page.getByRole("button", { name: rerun ? "Re-Run" : "Execute", exact: true }).click();
      const options = (await (await optionsResponse).json()).data;
      assert.equal(options.authority_source, "pack");
      assert.equal(options.policy_revision_id, r.policy_revision_id);
      assert.deepEqual(options.default_permission_set_refs, r.default_refs);
      assert.equal(options.default_selection_available, true);
      const dialog = page.getByRole("dialog", { name: rerun ? "Re-Run Action" : "Execute Action", exact: true });
      const nonce = rerun ? (custom ? r.old_explicit_nonce : r.old_nonce) : randomBytes(12).toString("hex");
      if (rerun) {
        assert.equal(await dialog.getByLabel(/^Probe nonce/).inputValue(), nonce);
        assert.equal(await dialog.getByLabel(/^Runtime action reference/).inputValue(), r.parameters.runtime_action_ref);
      } else {
        await dialog.getByLabel(/^Probe nonce/).fill(nonce);
        await dialog.getByLabel(/^Runtime action reference/).fill(r.parameters.runtime_action_ref);
      }
      await dialog.getByRole("button", { name: /Execution Token Access/ }).click();
      const selection = dialog.getByRole("radio", { name: mode === "default" ? /^Use action default/ : custom ? /^Specify permission sets/ : /^No execution API token/ });
      if (rerun) assert.equal(await selection.isChecked(), true);
      else await selection.check();
      if (custom) {
        await dialog.getByRole("button", { name: "Search and select permission sets...", exact: true }).click();
        for (const reference of r.explicit_refs) {
          const option = dialog.getByRole("option", { name: reference, exact: true });
          if (rerun) assert.equal(await option.getAttribute("aria-selected"), "true");
          else await option.click();
        }
        await dialog.getByRole("button", { name: "Search and select permission sets...", exact: true }).click();
      }
      const executionResponse = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/executions/execute" && response.request().method() === "POST");
      await dialog.getByRole("button", { name: rerun ? "Re-Run" : "Execute", exact: true }).click();
      const response = await executionResponse;
      assert.ok([200, 201, 202].includes(response.status()));
      const request = response.request().postDataJSON();
      if (mode === "default") assert.equal(Object.hasOwn(request, "permission_set_refs"), false);
      else assert.deepEqual(request.permission_set_refs, custom ? r.explicit_refs : []);
      const execution = (await response.json()).data;
      if (rerun) assert.notEqual(execution.id, oldExecution);
      assert.ok(executions.every((entry) => entry.id !== execution.id));
      executions.push({ id: execution.id, mode, nonce });
    }
    await confidentiality();
    return { real_browser: true, executions };
  } finally {
    try {
      await context.close();
    } finally {
      await browser.close();
    }
  }
}

try {
  let input = "";
  for await (const chunk of process.stdin) {
    input += chunk;
    if (input.length > 1024 * 1024) throw new Error("Input limit");
  }
  process.stdout.write(JSON.stringify(await main(JSON.parse(input))));
} catch {
  process.stderr.write("Real browser scenario failed; credentials, requests and page contents suppressed\n");
  process.exitCode = 1;
}
