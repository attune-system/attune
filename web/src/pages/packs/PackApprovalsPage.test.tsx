import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { PermissionsService } from "@/api/services/PermissionsService";
import type { RuntimeApprovalResponse } from "@/api/models/RuntimeApprovalResponse";
import PackApprovalsPage, { parseApprovalRequest } from "./PackApprovalsPage";

const approval = {
  id: 42,
  subject_pack_id: 7,
  issuer_identity_id: 3,
  issuer_login: "issuer@example.invalid",
  grants: [
    {
      resource: "events",
      actions: ["create"],
      constraints: { refs: ["shared.completed"] },
    },
  ],
  approval_digest: "a".repeat(64),
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
