import { afterEach, expect, it, vi } from "vitest";
import axios, { AxiosHeaders, type AxiosRequestConfig } from "axios";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import PackApprovalsPage from "./PackApprovalsPage";

const clients: QueryClient[] = [];
afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
  vi.restoreAllMocks();
});

const target = {
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
const draft = {
  grants: [
    {
      resource: "keys",
      actions: ["read"],
      scope: "external",
      constraints: { refs: ["shared.token"], encrypted: true },
    },
  ],
  expires_at: "2099-01-01T00:00:00Z",
};
const oldApproval = {
  id: 42,
  subject_pack_id: 7,
  issuer_identity_id: 3,
  issuer_login: "issuer@example.invalid",
  grants: draft.grants,
  approval_digest: "a".repeat(64),
  active: true,
  expires_at: draft.expires_at,
  revoked_at: null,
  purpose: { kind: "declared_input", targets: [target] },
};

function page(items: unknown[], current = 1, has_next = false) {
  return {
    items,
    pagination: {
      page: current,
      page_size: 20,
      has_next,
      has_previous: current > 1,
    },
  };
}

function response(data: unknown, status = 200) {
  return {
    data,
    status,
    statusText: status === 200 ? "OK" : "Forbidden",
    headers: {},
    config: { headers: new AxiosHeaders() },
  };
}

function transport(
  handler: (config: AxiosRequestConfig) => ReturnType<typeof response>,
) {
  return vi
    .spyOn(axios, "request")
    .mockImplementation(async (config) => handler(config));
}

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  clients.push(client);
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/packs/example/approvals"]}>
        <Routes>
          <Route path="/packs/:ref/approvals" element={<PackApprovalsPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

async function inputMode() {
  await userEvent.click(
    screen.getByRole("button", { name: "New input-only consent" }),
  );
  await screen.findByRole("button", { name: "Select input target 1" });
}

async function fillDraft() {
  const editor = screen.getByRole("textbox", {
    name: "Fixed grants and optional expiry",
  });
  await userEvent.clear(editor);
  await userEvent.paste(JSON.stringify(draft));
}

it.each([
  { items: [] },
  { items: [{ ...target, policy_revision: Number.MAX_SAFE_INTEGER + 1 }] },
  { items: [{ ...target, evaluated_value: "PRIVATE_VALUE" }] },
])(
  "empty or unverifiable discovery never enables fresh consent",
  async ({ items }) => {
    transport((config) =>
      response(
        url(config).pathname.endsWith("input-approval-targets")
          ? page(items)
          : page([]),
      ),
    );
    renderPage();
    await userEvent.click(
      screen.getByRole("button", { name: "New input-only consent" }),
    );
    await screen.findByText(
      items.length === 0
        ? /No currently visible retained targets/
        : /Target metadata cannot be verified/,
    );
    expect(
      screen.queryByRole("button", { name: /Select input target/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Issue input-only consent" }),
    ).toBeDisabled();
    expect(screen.queryByText(/PRIVATE_VALUE/)).not.toBeInTheDocument();
  },
);

it("replacement denial retains its immutable input target and confirmation without issuing fresh consent", async () => {
  const posts: AxiosRequestConfig[] = [];
  transport((config) => {
    if (config.method === "POST") {
      posts.push(config);
      return response({ error: "PRIVATE_VALUE source=/private" }, 403);
    }
    return response(page([oldApproval]));
  });
  renderPage();
  await userEvent.click(
    await screen.findByRole("button", { name: "Replace approval 42" }),
  );
  await userEvent.click(screen.getByRole("checkbox"));
  await userEvent.click(
    screen.getByRole("button", { name: "Replace and revoke previous" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Current covering grants",
  );
  expect(screen.getByRole("checkbox")).toBeChecked();
  expect(
    screen.getByRole("region", { name: "Replacement retained targets" }),
  ).toHaveTextContent(target.declaration_revision);
  expect(screen.queryByText(/PRIVATE_VALUE|\/private/)).not.toBeInTheDocument();
  expect(posts).toHaveLength(1);
  expect(url(posts[0]).pathname).toBe(
    "/api/v1/packs/example/external-approvals/42/replace",
  );
});

function wireBody(config: AxiosRequestConfig): unknown {
  const data: unknown = config.data;
  return typeof data === "string" ? JSON.parse(data) : data;
}

function url(config: AxiosRequestConfig): URL {
  return new URL(config.url ?? "", "http://localhost");
}

it("refreshes current-policy discovery without silently rebinding the selected target", async () => {
  let discoveries = 0;
  const updated = {
    ...target,
    policy_revision: 18,
    declaration_revision: "e".repeat(64),
  };
  transport((config) => {
    if (url(config).pathname.endsWith("input-approval-targets")) {
      discoveries += 1;
      return response(page([discoveries === 1 ? target : updated]));
    }
    return response(page([]));
  });
  renderPage();
  await inputMode();
  await userEvent.click(
    screen.getByRole("button", { name: "Select input target 1" }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Discover targets" }),
  );
  await waitFor(() => expect(discoveries).toBe(2));
  expect(
    screen.getByRole("region", { name: "Selected input target" }),
  ).toHaveTextContent(target.declaration_revision);
  await userEvent.click(
    await screen.findByRole("button", { name: "Select input target 1" }),
  );
  expect(
    screen.getByRole("region", { name: "Selected input target" }),
  ).toHaveTextContent(updated.declaration_revision);
});

it("selecting a second declaration replaces the choice, never combines targets or accepts a JSON rebind", async () => {
  const second = {
    ...target,
    source_path: "/input/other",
    expression: "{{ keystore['shared.other'] }}",
  };
  const posts: AxiosRequestConfig[] = [];
  transport((config) => {
    if (config.method === "POST") {
      posts.push(config);
      return response({ data: oldApproval });
    }
    return response(
      url(config).pathname.endsWith("input-approval-targets")
        ? page([target, second])
        : page([]),
    );
  });
  renderPage();
  await inputMode();
  await userEvent.click(
    screen.getByRole("button", { name: "Select input target 1" }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Select input target 2" }),
  );
  const editor = screen.getByRole("textbox", {
    name: "Fixed grants and optional expiry",
  });
  await userEvent.clear(editor);
  await userEvent.paste(
    JSON.stringify({ ...draft, policy_revision: 999, renderer_execution: 123 }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Issue input-only consent" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(/unknown fields/);
  expect(posts).toHaveLength(0);
  await fillDraft();
  await userEvent.click(
    screen.getByRole("button", { name: "Issue input-only consent" }),
  );
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(wireBody(posts[0])).toEqual({
    ...draft,
    policy_revision: second.policy_revision,
    invocation_binding: second.invocation_binding,
    declaration_revision: second.declaration_revision,
    source_path: second.source_path,
    expression: second.expression,
  });
});

it("discovery is visibility-filtered metadata, not pack association or consent", async () => {
  const requests: AxiosRequestConfig[] = [];
  transport((config) => {
    requests.push(config);
    return response(
      url(config).pathname.endsWith("input-approval-targets")
        ? page([target])
        : page([]),
    );
  });
  renderPage();
  await inputMode();
  expect(requests.every((request) => request.method === "GET")).toBe(true);
  expect(
    screen.getByText(
      /Pack association and discovery are not consent or a grant/,
    ),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Issue input-only consent" }),
  ).toBeDisabled();
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  await userEvent.click(
    screen.getByRole("button", { name: "Select input target 1" }),
  );
  expect(requests.every((request) => request.method === "GET")).toBe(true);
  expect(
    screen.queryByRole("button", { name: "Issue approval" }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByText(/Renderer issuance is not offered/),
  ).toBeInTheDocument();
});

it.each([
  { kind: "action", action_ref: "example.reader" },
  target.invocation_binding,
  { kind: "rule", rule_ref: "example.route", action_ref: "example.reader" },
  { kind: "queue", queue_ref: "example.jobs", action_ref: "example.reader" },
])(
  "issues exactly one flat policy target for $kind with no renderer or runtime request",
  async (invocation_binding) => {
    const posts: AxiosRequestConfig[] = [];
    transport((config) => {
      if (config.method === "POST") {
        posts.push(config);
        return response({ data: oldApproval });
      }
      return response(
        url(config).pathname.endsWith("input-approval-targets")
          ? page([{ ...target, invocation_binding }])
          : page([]),
      );
    });
    renderPage();
    await inputMode();
    await userEvent.click(
      screen.getByRole("button", { name: "Select input target 1" }),
    );
    await fillDraft();
    await userEvent.click(
      screen.getByRole("button", { name: "Issue input-only consent" }),
    );
    await waitFor(() => expect(posts).toHaveLength(1));
    expect(url(posts[0]).pathname).toBe(
      "/api/v1/packs/example/external-input-approvals",
    );
    expect(wireBody(posts[0])).toEqual({
      ...draft,
      policy_revision: target.policy_revision,
      invocation_binding,
      declaration_revision: target.declaration_revision,
      source_path: target.source_path,
      expression: target.expression,
    });
  },
);

it("issuer denial is redacted and retains the exact selected target, draft and mode", async () => {
  const posts: AxiosRequestConfig[] = [];
  transport((config) => {
    if (config.method === "POST") {
      posts.push(config);
      return response(
        {
          error: "PRIVATE_TOKEN source=/secret/config",
          details: { diagnostics: [{ message: "PRIVATE_TOKEN" }] },
        },
        403,
      );
    }
    return response(
      url(config).pathname.endsWith("input-approval-targets")
        ? page([target])
        : page([]),
    );
  });
  renderPage();
  await inputMode();
  await userEvent.click(
    screen.getByRole("button", { name: "Select input target 1" }),
  );
  await fillDraft();
  await userEvent.click(
    screen.getByRole("button", { name: "Issue input-only consent" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Current covering grants",
  );
  expect(
    screen.queryByText(/PRIVATE_TOKEN|\/secret\/config/),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("textbox", { name: "Fixed grants and optional expiry" }),
  ).toHaveValue(JSON.stringify(draft));
  expect(
    screen.getByRole("region", { name: "Selected input target" }),
  ).toHaveTextContent(target.declaration_revision);
  expect(
    screen.getByRole("heading", { name: "Issue input-only consent" }),
  ).toBeInTheDocument();
  await userEvent.click(
    screen.getByRole("button", { name: "Issue input-only consent" }),
  );
  await waitFor(() => expect(posts).toHaveLength(2));
  expect(wireBody(posts[1])).toEqual(wireBody(posts[0]));
});

it("paginates discovery, selects one retained revision, and never silently upgrades an old approval", async () => {
  const posts: AxiosRequestConfig[] = [];
  const gets: AxiosRequestConfig[] = [];
  const current = {
    ...target,
    policy_revision: 18,
    release_id: 66,
    declaration_revision: "e".repeat(64),
    expression: "{{ keystore['shared.new'] }}",
  };
  transport((config) => {
    if (config.method === "POST") {
      posts.push(config);
      return response({ data: oldApproval });
    }
    gets.push(config);
    if (url(config).pathname.endsWith("input-approval-targets")) {
      if (url(config).searchParams.get("policy_revision") === "17")
        return response(page([target]));
      return response(
        page(
          [current],
          Number(url(config).searchParams.get("page")),
          url(config).searchParams.get("page") === "1",
        ),
      );
    }
    return response(page([oldApproval]));
  });
  renderPage();
  await screen.findByText("Purpose: Input-only consent");
  await inputMode();
  await userEvent.click(
    screen.getByRole("button", { name: "Next target page" }),
  );
  await waitFor(() =>
    expect(gets.some((get) => url(get).searchParams.get("page") === "2")).toBe(
      true,
    ),
  );
  await userEvent.type(
    screen.getByRole("textbox", { name: /Retained policy revision/ }),
    "17",
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Discover targets" }),
  );
  await waitFor(() =>
    expect(
      gets.some(
        (get) =>
          url(get).searchParams.get("policy_revision") === "17" &&
          url(get).searchParams.get("page") === "1",
      ),
    ).toBe(true),
  );
  await userEvent.click(
    await screen.findByRole("button", { name: "Select input target 1" }),
  );
  expect(
    screen.getByRole("region", { name: "Selected input target" }),
  ).toHaveTextContent(target.declaration_revision);
  await userEvent.clear(
    screen.getByRole("textbox", { name: /Retained policy revision/ }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Discover targets" }),
  );
  await waitFor(() =>
    expect(screen.getByText(/Showing current policy/)).toBeInTheDocument(),
  );
  await userEvent.click(
    await screen.findByRole("button", { name: "Select input target 1" }),
  );
  expect(
    screen.getByRole("region", { name: "Selected input target" }),
  ).toHaveTextContent(current.declaration_revision);
  expect(
    screen.getByRole("region", { name: "Input targets for approval 42" }),
  ).toHaveTextContent(target.declaration_revision);
  await fillDraft();
  await userEvent.click(
    screen.getByRole("button", { name: "Issue input-only consent" }),
  );
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(wireBody(posts[0])).toEqual({
    ...draft,
    policy_revision: 18,
    invocation_binding: current.invocation_binding,
    declaration_revision: current.declaration_revision,
    source_path: current.source_path,
    expression: current.expression,
  });
  expect(posts[0].url).not.toContain("replace");
});

it("input replacement confirms predecessor revocation and sends only grants/expiry to the purpose-preserving route", async () => {
  const posts: AxiosRequestConfig[] = [];
  transport((config) => {
    if (config.method === "POST") {
      posts.push(config);
      return response({ data: { ...oldApproval, id: 43 } });
    }
    return response(page([oldApproval]));
  });
  renderPage();
  await userEvent.click(
    await screen.findByRole("button", { name: "Replace approval 42" }),
  );
  expect(
    screen.getByRole("heading", { name: "Replace input-only approval 42" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("region", { name: "Replacement retained targets" }),
  ).toHaveTextContent(target.declaration_revision);
  expect(
    screen.queryByRole("button", { name: /Select input target/ }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Replace and revoke previous" }),
  ).toBeDisabled();
  expect(posts).toHaveLength(0);
  await userEvent.click(screen.getByRole("checkbox"));
  await userEvent.click(
    screen.getByRole("button", { name: "Replace and revoke previous" }),
  );
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(url(posts[0]).pathname).toBe(
    "/api/v1/packs/example/external-approvals/42/replace",
  );
  expect(wireBody(posts[0])).toEqual(draft);
});

it("runtime issuance and replacement keep their existing generated service endpoints and bodies", async () => {
  const posts: AxiosRequestConfig[] = [];
  transport((config) => {
    if (config.method === "POST") {
      posts.push(config);
      return response({ data: oldApproval });
    }
    return response(page([{ ...oldApproval, purpose: { kind: "runtime" } }]));
  });
  renderPage();
  await screen.findByText("Purpose: Runtime API authority");
  await fillDraft();
  await userEvent.click(screen.getByRole("button", { name: "Issue approval" }));
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(url(posts[0]).pathname).toBe(
    "/api/v1/packs/example/external-approvals",
  );
  expect(wireBody(posts[0])).toEqual(draft);
  await userEvent.click(
    await screen.findByRole("button", { name: "Replace approval 42" }),
  );
  await userEvent.click(screen.getByRole("checkbox"));
  await userEvent.click(
    screen.getByRole("button", { name: "Replace and revoke previous" }),
  );
  await waitFor(() => expect(posts).toHaveLength(2));
  expect(url(posts[1]).pathname).toBe(
    "/api/v1/packs/example/external-approvals/42/replace",
  );
  expect(wireBody(posts[1])).toEqual(draft);
});
