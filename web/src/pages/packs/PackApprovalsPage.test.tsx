import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import axios, { AxiosHeaders } from "axios";
import { PermissionsService } from "@/api/services/PermissionsService";
import type { RuntimeApprovalResponse } from "@/api/models/RuntimeApprovalResponse";
import { RbacAction } from "@/api/models/RbacAction";
import { RbacResource } from "@/api/models/RbacResource";
import PackApprovalsPage from "./PackApprovalsPage";
import { parseApprovalRequest } from "./approval-request";

const approval = {
  id: 42,
  subject_pack_id: 7,
  issuer_identity_id: 3,
  issuer_login: "issuer@example.invalid",
  grants: [
    {
      resource: RbacResource.EVENTS,
      actions: [RbacAction.CREATE],
      constraints: { refs: ["shared.completed"] },
    },
  ],
  approval_digest: "a".repeat(64),
  purpose: { kind: "runtime" },
  active: true,
  expires_at: null,
  revoked_at: null,
} satisfies RuntimeApprovalResponse;
const clients: QueryClient[] = [];
afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
  vi.restoreAllMocks();
});

function renderControls() {
  vi.spyOn(PermissionsService, "listRuntimeApprovals").mockResolvedValue({
    items: [approval],
    pagination: {
      page: 1,
      page_size: 25,
      has_next: false,
      has_previous: false,
    },
  });
  return renderPage();
}

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  clients.push(client);
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/packs/example/approvals"]}>
        <Routes>
          <Route path="/packs/:ref/approvals" element={<PackApprovalsPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const visibleInputTarget = {
  policy_revision: 17,
  invocation_binding: {
    kind: "workflow_task",
    workflow_ref: "example.flow",
    task_path: "/tasks/~1named",
    action_ref: "example.reader",
  },
  action_id: 33,
  action_ref: "example.reader",
  pack_id: 7,
  pack_ref: "example",
  release_id: 44,
  metadata_revision: 55,
  declaration_revision: "d".repeat(64),
  source_path: "/input/token",
  expression: "{{ keystore['shared.token'] }}",
};

function renderWirePurpose(purpose: unknown) {
  // Exercise the actual generated service, including malformed wire metadata.
  vi.spyOn(axios, "request").mockResolvedValue({
    status: 200,
    statusText: "OK",
    headers: {},
    config: { headers: new AxiosHeaders() },
    data: {
      items: [{ ...approval, purpose }],
      pagination: {
        page: 1,
        page_size: 25,
        has_next: false,
        has_previous: false,
      },
    },
  });
  return renderPage();
}

it("shows exact input-only targets but never offers them in the runtime editor", async () => {
  const issue = vi.spyOn(PermissionsService, "issueRuntimeApproval");
  const replace = vi.spyOn(PermissionsService, "replaceRuntimeApproval");
  renderWirePurpose({ kind: "declared_input", targets: [visibleInputTarget] });
  expect(
    await screen.findByText("Purpose: Input-only consent"),
  ).toBeInTheDocument();
  const targets = screen.getByRole("region", {
    name: "Input targets for approval 42",
  });
  const displayed: unknown = JSON.parse(
    targets.querySelector("pre")?.textContent ?? "",
  );
  expect(displayed).toEqual(visibleInputTarget);
  expect(
    screen.getByRole("button", { name: "Replace approval 42" }),
  ).toBeEnabled();
  expect(
    screen.getByRole("heading", { name: "Issue runtime approval" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("textbox")).toHaveValue(
    '{\n  "grants": [],\n  "expires_at": null\n}',
  );
  expect(issue).not.toHaveBeenCalled();
  expect(replace).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "Replace approval 42" }),
  );
  expect(
    screen.getByRole("heading", { name: "Replace input-only approval 42" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("region", { name: "Replacement retained targets" }),
  ).toHaveTextContent("Replacement preserves these declarations");
  expect(
    screen.queryByRole("heading", { name: "Replace runtime approval 42" }),
  ).not.toBeInTheDocument();
});

it("keeps input purpose when all retained targets are hidden or collected", async () => {
  renderWirePurpose({ kind: "declared_input", targets: [] });
  expect(
    await screen.findByText("Purpose: Input-only consent"),
  ).toBeInTheDocument();
  expect(
    screen.getByText(/No currently visible retained targets/),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Replace approval 42" }),
  ).not.toBeInTheDocument();
});

it("fails closed on missing or malformed purpose instead of guessing runtime", async () => {
  renderWirePurpose(undefined);
  expect(
    await screen.findByText("Purpose: Purpose unavailable"),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Replace approval 42" }),
  ).not.toBeInTheDocument();
});

it("keeps malformed input metadata out of runtime controls and does not render extra values", async () => {
  renderWirePurpose({
    kind: "declared_input",
    targets: [
      { ...visibleInputTarget, evaluated_value: "never-render-this-value" },
    ],
  });
  expect(
    await screen.findByText("Purpose: Input-only consent"),
  ).toBeInTheDocument();
  expect(
    screen.getByText(/Target metadata is unavailable/),
  ).toBeInTheDocument();
  expect(screen.queryByText(/never-render-this-value/)).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Replace approval 42" }),
  ).not.toBeInTheDocument();
});

it("rechecks input-only revocation confirmation without issuing runtime authority", async () => {
  const revoke = vi
    .spyOn(PermissionsService, "revokeRuntimeApproval")
    .mockResolvedValue({ data: approval });
  const issue = vi.spyOn(PermissionsService, "issueRuntimeApproval");
  renderWirePurpose({ kind: "declared_input", targets: [visibleInputTarget] });
  await userEvent.click(
    await screen.findByRole("button", { name: "Revoke approval 42" }),
  );
  const dialog = screen.getByRole("dialog");
  expect(dialog).toHaveTextContent("Purpose: Input-only consent");
  expect(dialog).toHaveTextContent("not a runtime permission selection");
  expect(revoke).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "Confirm revocation" }),
  );
  await waitFor(() =>
    expect(revoke).toHaveBeenCalledWith({ packRef: "example", id: 42 }),
  );
  expect(issue).not.toHaveBeenCalled();
});

it("requires confirmation before a replacement revokes old authority", async () => {
  const replace = vi
    .spyOn(PermissionsService, "replaceRuntimeApproval")
    .mockResolvedValue({ data: { ...approval, id: 43 } });
  renderControls();
  await userEvent.click(
    await screen.findByRole("button", { name: "Replace approval 42" }),
  );
  const save = screen.getByRole("button", {
    name: "Replace and revoke previous",
  });
  expect(save).toBeDisabled();
  expect(replace).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("checkbox"));
  await userEvent.click(save);
  await waitFor(() =>
    expect(replace).toHaveBeenCalledWith({
      packRef: "example",
      id: 42,
      requestBody: { grants: approval.grants, expires_at: null },
    }),
  );
});

it("only sends revocation after explicit confirmation", async () => {
  const revoke = vi
    .spyOn(PermissionsService, "revokeRuntimeApproval")
    .mockResolvedValue({
      data: { ...approval, active: false, revoked_at: "2026-10-14T00:00:00Z" },
    });
  renderControls();
  await userEvent.click(
    await screen.findByRole("button", { name: "Revoke approval 42" }),
  );
  expect(
    screen.getByRole("dialog", { name: "Confirm approval revocation" }),
  ).toBeInTheDocument();
  expect(revoke).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "Confirm revocation" }),
  );
  await waitFor(() =>
    expect(revoke).toHaveBeenCalledWith({ packRef: "example", id: 42 }),
  );
});

describe("approval request validation", () => {
  it("accepts explicit grants without creating personal IAM assignments", () => {
    const grants = [
      {
        resource: "events",
        actions: ["create"],
        constraints: { refs: ["shared.completed"] },
      },
    ];
    expect(parseApprovalRequest(JSON.stringify({ grants }))).toEqual({
      grants,
      expires_at: null,
    });
  });
  it("rejects invalid boundary data", () => {
    for (const value of [
      null,
      [],
      { grants: [] },
      { grants: [null] },
      { grants: [{ resource: "events", actions: [] }] },
      { grants: [{ resource: "events", actions: [42] }] },
      { grants: [{}], unexpected: true },
    ]) {
      expect(() => parseApprovalRequest(JSON.stringify(value))).toThrow();
    }
  });
  it("rejects expired or malformed expiry without silently removing it", () => {
    const grants = [{ resource: "events", actions: ["create"] }];
    for (const expires_at of [
      42,
      "not-a-date",
      "01/01/2099",
      "2000-01-01T00:00:00Z",
    ]) {
      expect(() =>
        parseApprovalRequest(JSON.stringify({ grants, expires_at })),
      ).toThrow();
    }
  });
});
