import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  cleanup,
} from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import axios from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import ExecutionArtifactsPanel from "@/components/executions/ExecutionArtifactsPanel";
import { queryClient } from "@/lib/query-client";
import ArtifactDetailPage from "./ArtifactDetailPage";
import ArtifactsPage from "./ArtifactsPage";

vi.mock("@/contexts/WebSocketContext", () => ({
  useWebSocketContext: () => ({ connected: true }),
  useEntityNotifications: () => ({ connected: true }),
}));

const header = {
  id: 42,
  ref: "core.echo.stdout.log",
  name: "stdout",
  classification: "runtime_log",
  type: "file_text",
  visibility: "private",
  scope: "action",
  owner: "core.echo",
  created: "2026-10-30T12:00:00Z",
  updated: "2026-10-30T12:00:00Z",
  retention_policy: "versions",
  retention_limit: 10,
};
const selected = {
  id: 71,
  version: 7,
  execution: 10,
  content_type: "text/plain",
  size_bytes: 70000,
  created: header.created,
  created_by: null,
};
const page = (items: unknown[], next_cursor: string | null = null) => ({
  items,
  next_cursor,
  has_more: next_cursor !== null,
});
const clients: QueryClient[] = [];
function mount(ui: React.ReactNode, route = "/") {
  const client = new QueryClient({
    defaultOptions: queryClient.getDefaultOptions(),
  });
  clients.push(client);
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[route]}>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}
afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.removeItem("access_token");
});

describe("artifact browsing through production hooks", () => {
  it("previews the execution-selected version with a bounded range, never the shared latest", async () => {
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const fetch = vi.fn().mockResolvedValue(new Response("run A content"));
    vi.stubGlobal("fetch", fetch);
    mount(<ExecutionArtifactsPanel executionId={10} />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByText("run A content");
    expect(fetch).toHaveBeenCalledExactlyOnceWith(
      expect.stringContaining(
        "/artifacts/42/versions/7/download?version_id=71",
      ),
      expect.objectContaining({
        headers: expect.objectContaining({ Range: "bytes=0-65535" }),
        signal: expect.any(AbortSignal),
      }),
    );
    expect(
      screen.getByRole("heading", { name: /Execution #10.*v7/ }),
    ).toBeInTheDocument();
  });

  it("starts history from the route ID without waiting for the header and keeps the header on denial", async () => {
    let releaseHeader!: (value: unknown) => void;
    const pending = new Promise((resolve) => {
      releaseHeader = resolve;
    });
    const send = vi
      .spyOn(axios, "request")
      .mockImplementation(async (config) => {
        if (config.url?.includes("/versions"))
          return { status: 404, data: {}, headers: {} };
        return pending;
      });
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(new Response("private reason", { status: 404 })),
    );
    mount(
      <Routes>
        <Route path="/artifacts/:id" element={<ArtifactDetailPage />} />
      </Routes>,
      "/artifacts/42?execution=10&version=7&version_id=71",
    );
    await waitFor(() =>
      expect(
        send.mock.calls.some(([config]) =>
          config.url?.includes("/artifacts/42/versions"),
        ),
      ).toBe(true),
    );
    releaseHeader({ status: 200, data: { data: header }, headers: {} });
    expect(
      await screen.findByRole("heading", { name: "stdout" }),
    ).toBeInTheDocument();
    await screen.findByText("Content unavailable or access denied");
    await screen.findByText("Failed to load versions");
    expect(
      send.mock.calls.filter(([config]) => config.url?.includes("/versions")),
    ).toHaveLength(1);
    expect(screen.queryByText("private reason")).not.toBeInTheDocument();
  });

  it("browses empty resumable cursor pages and returns to the first page on resume", async () => {
    const send = vi
      .spyOn(axios, "request")
      .mockImplementation(async (config) => ({
        status: 200,
        headers: {},
        data: config.url?.includes("cursor=older")
          ? page([], "oldest")
          : page([header], "older"),
      }));
    mount(<ArtifactsPage />);
    await screen.findByRole("link", { name: "stdout" });
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("No artifacts found");
    expect(screen.getByRole("button", { name: "Next" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Previous" })).toBeEnabled();
    expect(send.mock.calls.at(-1)?.[0].url).toContain("cursor=older");
    expect(send.mock.calls.at(-1)?.[0].url).not.toMatch(/[?&]page=/);
    fireEvent.click(screen.getByRole("button", { name: "Previous" }));
    await screen.findByRole("link", { name: "stdout" });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("No artifacts found");
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    await screen.findByRole("link", { name: "stdout" });
    expect(screen.getByRole("button", { name: "Previous" })).toBeDisabled();
    expect(send.mock.calls.at(-1)?.[0].url).not.toContain("cursor=");
  });

  it("streams only the selected execution, resumes at SSE byte IDs, and preserves multiline whitespace", async () => {
    localStorage.setItem("access_token", "test-token");
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(
          "id: 12\nevent: content\ndata:   run A\ndata: line two\n\n",
        ),
      )
      .mockResolvedValueOnce(
        new Response(
          "id: 21\nevent: content\ndata: tail\n\nid: 21\nevent: done\ndata: verified\n\n",
        ),
      );
    vi.stubGlobal("fetch", fetch);
    mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByText("Stream complete", {}, { timeout: 2500 });
    expect(fetch.mock.calls[0][0]).toMatch(
      /\/executions\/10\/logs\/stdout\/stream$/,
    );
    expect(fetch.mock.calls[1][0]).toMatch(
      /\/executions\/10\/logs\/stdout\/stream\?offset=12$/,
    );
    expect(screen.getByText(/run A/).textContent).toBe("  run A\nline twotail");
    expect(fetch).toHaveBeenCalledTimes(2);
    localStorage.removeItem("access_token");
  });

  it.each(["stdout", "stderr"])(
    "uses execution SSE, not finite download, for running owner-bound recorded %s",
    async (stream) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([
          {
            ...header,
            ref: `core.echo.owner_481.${stream}.log`,
            selected_version: selected,
          },
        ]),
        headers: {},
      });
      const fetch = vi
        .fn()
        .mockImplementation(async (url: string) =>
          url.endsWith(`/executions/10/logs/${stream}/stream`)
            ? new Response(
                "id: 9\nevent: content\ndata: recorded log\n\nid: 9\nevent: done\ndata: verified\n\n",
              )
            : new Response("Unsealed log", { status: 404 }),
        );
      vi.stubGlobal("fetch", fetch);
      mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
      expect(fetch.mock.calls[0][0]).toMatch(
        new RegExp(`/executions/10/logs/${stream}/stream$`),
      );
      await screen.findByText("recorded log");
      await screen.findByText("Stream complete");
      expect(
        fetch.mock.calls.some(([url]) => String(url).includes("/download")),
      ).toBe(false);
    },
  );

  it("downloads only the execution-selected version and reports content denial without hiding metadata", async () => {
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const fetch = vi
      .fn()
      .mockResolvedValue(new Response("private reason", { status: 404 }));
    vi.stubGlobal("fetch", fetch);
    mount(<ExecutionArtifactsPanel executionId={10} />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Download" }));
    await screen.findByText("Content unavailable or access denied");
    expect(fetch).toHaveBeenCalledExactlyOnceWith(
      expect.stringContaining(
        "/artifacts/42/versions/7/download?version_id=71",
      ),
      expect.anything(),
    );
    expect(screen.getByRole("link", { name: "View details" })).toHaveAttribute(
      "href",
      "/artifacts/42?execution=10&version=7&version_id=71",
    );
  });

  it.each(["execution=10", "execution=10&version=wrong"])(
    "never falls back to latest for an incomplete selection: %s",
    async (selection) => {
      vi.spyOn(axios, "request").mockImplementation(async (config) => ({
        status: 200,
        headers: {},
        data: config.url?.includes("/versions") ? page([]) : { data: header },
      }));
      const fetch = vi.fn().mockResolvedValue(new Response("wrong latest"));
      vi.stubGlobal("fetch", fetch);
      mount(
        <Routes>
          <Route path="/artifacts/:id" element={<ArtifactDetailPage />} />
        </Routes>,
        `/artifacts/42?${selection}`,
      );
      await screen.findByRole("heading", { name: "stdout" });
      await screen.findByText("Content unavailable or access denied");
      expect(fetch).not.toHaveBeenCalled();
      expect(
        screen.queryByRole("button", { name: /Download Latest/ }),
      ).not.toBeInTheDocument();
    },
  );

  it("cancels and releases a running execution log reader on close", async () => {
    localStorage.setItem("access_token", "test-token");
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const cancel = vi.fn();
    const body = new ReadableStream({ cancel });
    const fetch = vi.fn().mockResolvedValue(new Response(body));
    vi.stubGlobal("fetch", fetch);
    mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(body.locked).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "Close preview" }));
    await waitFor(() => expect(cancel).toHaveBeenCalledOnce());
    await waitFor(() => expect(body.locked).toBe(false));
    expect(fetch.mock.calls[0][1].signal.aborted).toBe(true);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("does not reconnect a guarded SSE denial or disclose its private reason", async () => {
    localStorage.setItem("access_token", "test-token");
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(
          "event: error\ndata: private reason\n\nevent: append\ndata: forbidden tail\n\n",
        ),
      );
    vi.stubGlobal("fetch", fetch);
    mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByText(/Content unavailable or access denied/);
    expect(fetch.mock.calls[0][1].signal.aborted).toBe(true);
    expect(fetch).toHaveBeenCalledOnce();
    expect(
      screen.queryByText(/private reason|forbidden tail/),
    ).not.toBeInTheDocument();
  });

  it.each([
    { classification: "general", ref: "core.echo.owner_481.stdout.log" },
    { ref: "other.echo.owner_481.stdout.log" },
    { ref: "core.echo.extra.owner_481.stdout.log" },
    { ref: "core.echo.owner_481.stdout.log.extra" },
    { ref: "core.echo.owner_0.stdout.log" },
    { scope: "sensor", ref: "core.echo.owner_481.stdout.log" },
  ])(
    "does not treat a noncanonical or unrelated artifact as execution stdout: %j",
    async (fields) => {
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, ...fields, selected_version: selected }]),
        headers: {},
      });
      const fetch = vi.fn().mockResolvedValue(new Response("selected file"));
      vi.stubGlobal("fetch", fetch);
      mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText("selected file");
      expect(fetch.mock.calls[0][0]).toContain(
        "/artifacts/42/versions/7/download",
      );
    },
  );

  it("keeps the immutable selected ID when a retained-away version number is reused", async () => {
    vi.spyOn(axios, "request").mockImplementation(async (config) => ({
      status: 200,
      headers: {},
      data: config.url?.includes("/versions")
        ? page([{ ...selected, id: 99, execution: 20 }])
        : { data: header },
    }));
    const fetch = vi
      .fn()
      .mockResolvedValue(new Response("denied", { status: 404 }));
    vi.stubGlobal("fetch", fetch);
    mount(
      <Routes>
        <Route path="/artifacts/:id" element={<ArtifactDetailPage />} />
      </Routes>,
      "/artifacts/42?execution=10&version=7&version_id=71",
    );
    await screen.findByText("Content unavailable or access denied");
    expect(fetch).toHaveBeenCalledExactlyOnceWith(
      expect.stringContaining("/versions/7/download?version_id=71"),
      expect.anything(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Preview content" }));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
    expect(fetch.mock.calls[1][0]).toContain(
      "/versions/7/download?version_id=99",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Download this version" }),
    );
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(3));
    expect(fetch.mock.calls[2][0]).toContain(
      "/versions/7/download?version_id=99",
    );
  });
});
