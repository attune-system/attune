import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  cleanup,
  act,
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
function logFrames(chunks: string[], start = 0) {
  let offset = start;
  const wire = chunks
    .map((text, index) => {
      offset += new TextEncoder().encode(text).length;
      return `id: ${offset}\nevent: ${index === 0 ? "content" : "append"}\n${text
        .split("\n")
        .map((line) => `data: ${line}\n`)
        .join("")}\n`;
    })
    .join("");
  return { wire, offset };
}
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
  it("shows incomplete capture metadata without requesting private log bodies", async () => {
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([]),
      headers: {},
    });
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    mount(<ExecutionArtifactsPanel executionId={10} logsIncomplete />);
    await screen.findByText("Log delivery is incomplete for this execution.");
    expect(fetch).not.toHaveBeenCalled();
  });

  it.each([
    "200 KiB ASCII",
    "1 MiB newline expansion",
    "1 MiB lossy UTF-8 expansion",
  ])(
    "accepts a fragmented legacy frame containing %s with bounded display",
    async (kind) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, selected_version: selected }]),
        headers: {},
      });
      const newline = kind === "1 MiB newline expansion";
      const lossy = kind === "1 MiB lossy UTF-8 expansion";
      const size = newline || lossy ? 1024 * 1024 : 200 * 1024;
      const wire = new TextEncoder().encode(
        `id: ${size}\nevent: content\n${newline ? "data: \n".repeat(size + 1) : `data: ${(lossy ? "�" : "x").repeat(size)}\n`}\nevent: done\ndata: Log stream sealed\n\n`,
      );
      let position = 0;
      const body = new ReadableStream<Uint8Array>({
        pull(controller) {
          if (position === wire.length) {
            controller.close();
            return;
          }
          const end = Math.min(wire.length, position + 8191);
          controller.enqueue(wire.subarray(position, end));
          position = end;
        },
      });
      const fetch = vi.fn().mockResolvedValue(new Response(body));
      vi.stubGlobal("fetch", fetch);
      const view = mount(
        <ExecutionArtifactsPanel executionId={10} isRunning />,
      );
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText("Stream complete", {}, { timeout: 4000 });
      const text = view.container.querySelector("pre")?.textContent;
      expect(new TextEncoder().encode(text ?? "").length).toBe(
        lossy ? 65535 : 65536,
      );
      expect(
        text ===
          (lossy ? "�".repeat(21845) : (newline ? "\n" : "x").repeat(65536)),
      ).toBe(true);
      expect(
        screen.queryByText(/Content unavailable or access denied/),
      ).not.toBeInTheDocument();
      expect(fetch).toHaveBeenCalledOnce();
    },
  );

  it.each([
    "empty legacy",
    "nonempty legacy",
    "empty protected",
    "protected equal cursor",
  ])(
    "completes %s logs without inventing another content cursor",
    async (kind) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, selected_version: selected }]),
        headers: {},
      });
      const empty = kind === "empty legacy" || kind === "empty protected";
      const data = empty
        ? ""
        : "id: 5\nevent: content\ndata: hello\n\n" +
          // Legacy SendInitial emits `content` for every catch-up chunk.
          (kind === "nonempty legacy"
            ? "id: 11\nevent: content\ndata:  world\n\n"
            : "");
      const doneId =
        kind === "protected equal cursor"
          ? "id: 5\n"
          : kind === "empty protected"
            ? "id: 0\n"
            : "";
      const fetch = vi
        .fn()
        .mockResolvedValue(
          new Response(
            `${data}${doneId}event: done\ndata: Log stream sealed\n\n`,
          ),
        );
      vi.stubGlobal("fetch", fetch);
      const view = mount(
        <ExecutionArtifactsPanel executionId={10} isRunning />,
      );
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText("Stream complete");
      expect(view.container.querySelector("pre")?.textContent).toBe(
        empty
          ? "(empty)"
          : kind === "nonempty legacy"
            ? "hello world"
            : "hello",
      );
      expect(
        screen.queryByText(
          /Content unavailable or access denied|Loading content/,
        ),
      ).not.toBeInTheDocument();
      expect(fetch).toHaveBeenCalledOnce();
    },
  );

  it.each([
    { event: "append", id: "100", reconnect: false },
    { event: "append", id: "90", reconnect: false },
    { event: "content", id: "100", reconnect: true },
    { event: "content", id: "90", reconnect: true },
  ])(
    "rejects non-increasing $event cursor $id before rendering its data",
    async ({ event, id, reconnect }) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, selected_version: selected }]),
        headers: {},
      });
      const first = `id: 100\nevent: content\ndata: ${"x".repeat(100)}\n\n`;
      const invalid = `id: ${id}\nevent: ${event}\ndata: duplicated or rewound content\n\n`;
      const cancel = vi.fn();
      const body = new ReadableStream({
        start(controller) {
          controller.enqueue(
            new TextEncoder().encode((reconnect ? "" : first) + invalid),
          );
        },
        cancel,
      });
      const fetch = reconnect
        ? vi
            .fn()
            .mockResolvedValueOnce(new Response(first))
            .mockResolvedValueOnce(new Response(body))
        : vi.fn().mockResolvedValue(new Response(body));
      vi.stubGlobal("fetch", fetch);
      const view = mount(
        <ExecutionArtifactsPanel executionId={10} isRunning />,
      );
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText(
        /Content unavailable or access denied/,
        {},
        { timeout: 2500 },
      );
      await waitFor(() => expect(body.locked).toBe(false));
      expect(cancel).toHaveBeenCalledOnce();
      expect(fetch).toHaveBeenCalledTimes(reconnect ? 2 : 1);
      if (reconnect) expect(fetch.mock.calls[1][0]).toMatch(/\?offset=100$/);
      expect(fetch.mock.calls.at(-1)?.[1].signal.aborted).toBe(true);
      expect(view.container.querySelector("pre")).toBeNull();
      expect(
        screen.queryByText("duplicated or rewound content"),
      ).not.toBeInTheDocument();
    },
  );

  it.each([
    {
      label: "ASCII whitespace",
      chunks: [...Array<string>(12).fill("x".repeat(16384)), " \tend\n"],
      expected: "x".repeat(65530) + " \tend\n",
    },
    {
      label: "multibyte boundaries",
      chunks: [...Array<string>(12).fill("界".repeat(5461)), "😀 \t"],
      expected: "界".repeat(21843) + "😀 \t",
    },
  ])(
    "bounds running preview to a UTF-8 tail preserving $label",
    async ({ chunks, expected }) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, selected_version: selected }]),
        headers: {},
      });
      const { wire, offset } = logFrames(chunks);
      const fetch = vi
        .fn()
        .mockResolvedValue(
          new Response(`${wire}id: ${offset}\nevent: done\ndata: verified\n\n`),
        );
      vi.stubGlobal("fetch", fetch);
      const view = mount(
        <ExecutionArtifactsPanel executionId={10} isRunning />,
      );
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText("Stream complete");
      const text = view.container.querySelector("pre")?.textContent;
      expect(text).toBe(expected);
      expect(new TextEncoder().encode(text ?? "").length).toBeLessThanOrEqual(
        65536,
      );
      expect(text).not.toContain("�");
      expect(
        screen.getByText(/Live preview limited to the most recent 64 KiB/),
      ).toBeInTheDocument();
      expect(fetch).toHaveBeenCalledOnce();
    },
  );

  it("resumes content frames at the complete server offset after the displayed prefix is dropped", async () => {
    localStorage.setItem("access_token", "test-token");
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const first = logFrames(Array<string>(5).fill("x".repeat(16384)));
    const second = logFrames([" \ttail\n"], first.offset);
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response(first.wire))
      .mockResolvedValueOnce(
        new Response(
          `${second.wire}id: ${second.offset}\nevent: done\ndata: verified\n\n`,
        ),
      );
    vi.stubGlobal("fetch", fetch);
    const view = mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByText("Stream complete", {}, { timeout: 2500 });
    expect(fetch.mock.calls[1][0]).toMatch(
      /\/executions\/10\/logs\/stdout\/stream\?offset=81920$/,
    );
    const text = view.container.querySelector("pre")?.textContent;
    expect(text?.length).toBe(65536);
    expect(text?.endsWith(" \ttail\n")).toBe(true);
    expect(
      screen.getByText(/Live preview limited to the most recent 64 KiB/),
    ).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("preserves a complete server u64 resume ID without rounding or truncating it", async () => {
    localStorage.setItem("access_token", "test-token");
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response("id: 18446744073709551614\nevent: content\ndata: x\n\n"),
      )
      .mockResolvedValueOnce(
        new Response(
          "id: 18446744073709551615\nevent: content\ndata: y\n\nid: 18446744073709551615\nevent: done\ndata: verified\n\n",
        ),
      );
    vi.stubGlobal("fetch", fetch);
    const view = mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByText("Stream complete", {}, { timeout: 2500 });
    expect(fetch.mock.calls[1][0]).toMatch(/\?offset=18446744073709551614$/);
    expect(view.container.querySelector("pre")?.textContent).toBe("xy");
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("keeps a fresh reconnect denial authoritative over any previously retained preview", async () => {
    localStorage.setItem("access_token", "test-token");
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    const first = logFrames(Array<string>(5).fill("x".repeat(16384)));
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response(first.wire))
      .mockResolvedValueOnce(
        new Response("private revocation reason", { status: 403 }),
      );
    vi.stubGlobal("fetch", fetch);
    const view = mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByText(/Live preview limited to the most recent 64 KiB/);
    await screen.findByText(
      /Content unavailable or access denied/,
      {},
      { timeout: 2500 },
    );
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(fetch.mock.calls[1][0]).toMatch(
      /\/executions\/10\/logs\/stdout\/stream\?offset=81920$/,
    );
    expect(fetch.mock.calls[1][1].signal.aborted).toBe(true);
    expect(view.container.querySelector("pre")).toBeNull();
    expect(
      screen.queryByText("private revocation reason"),
    ).not.toBeInTheDocument();
  });

  it.each([
    "oversized",
    "invalid-id",
    "missing-id",
    "missing-data",
    "invalid-utf8",
  ])(
    "stops and disposes a %s SSE frame without retry or selected-sibling fallback",
    async (kind) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, selected_version: selected }]),
        headers: {},
      });
      const encode = (text: string) => new TextEncoder().encode(text);
      const bytes =
        kind === "oversized"
          ? encode(
              `id: 9\nevent: content\ndata: ${"x".repeat(8 * 1024 * 1024 + 1)}`,
            )
          : kind === "missing-id"
            ? encode("event: content\ndata: private frame\n\n")
            : kind === "missing-data"
              ? encode("id: 9\nevent: content\n\n")
              : kind === "invalid-id"
                ? encode(
                    "id: not-an-offset\nevent: content\ndata: private frame\n\n",
                  )
                : new Uint8Array([
                    ...encode("id: 9\nevent: content\ndata: "),
                    255,
                    ...encode("\n\n"),
                  ]);
      const cancel = vi.fn();
      const body = new ReadableStream({
        start(controller) {
          controller.enqueue(bytes);
        },
        cancel,
      });
      const fetch = vi.fn().mockResolvedValue(new Response(body));
      vi.stubGlobal("fetch", fetch);
      mount(<ExecutionArtifactsPanel executionId={10} isRunning />);
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText(/Content unavailable or access denied/);
      await waitFor(() => expect(body.locked).toBe(false));
      expect(cancel).toHaveBeenCalledOnce();
      expect(fetch.mock.calls[0][1].signal.aborted).toBe(true);
      expect(fetch).toHaveBeenCalledOnce();
      expect(screen.queryByText("private frame")).not.toBeInTheDocument();
    },
  );

  it.each(["revocation", "unmount"])(
    "disposes a truncated active preview on %s without changing its selected execution",
    async (reason) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, selected_version: selected }]),
        headers: {},
      });
      const first = logFrames(Array<string>(5).fill("x".repeat(16384)));
      let writer!: ReadableStreamDefaultController<Uint8Array>;
      const cancel = vi.fn();
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          writer = controller;
          controller.enqueue(new TextEncoder().encode(first.wire));
        },
        cancel,
      });
      const fetch = vi.fn().mockResolvedValue(new Response(body));
      vi.stubGlobal("fetch", fetch);
      const view = mount(
        <ExecutionArtifactsPanel executionId={10} isRunning />,
      );
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText(/Live preview limited to the most recent 64 KiB/);
      if (reason === "unmount") view.unmount();
      else {
        await act(async () =>
          writer.enqueue(
            new TextEncoder().encode(
              "event: error\ndata: private revocation reason\n\nid: 81926\nevent: append\ndata: sibling content\n\n",
            ),
          ),
        );
        await screen.findByText(/Content unavailable or access denied/);
        expect(
          screen.queryByText(/private revocation reason|sibling content/),
        ).not.toBeInTheDocument();
        expect(view.container.querySelector("pre")).toBeNull();
      }
      await waitFor(() => expect(body.locked).toBe(false));
      expect(cancel).toHaveBeenCalledOnce();
      expect(fetch).toHaveBeenCalledOnce();
      expect(fetch.mock.calls[0][0]).toMatch(
        /\/executions\/10\/logs\/stdout\/stream$/,
      );
      expect(fetch.mock.calls[0][1].signal.aborted).toBe(true);
    },
  );

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
      expect(
        fetch.mock.calls[0][0].endsWith(`/executions/10/logs/${stream}/stream`),
      ).toBe(true);
      await screen.findByText("recorded log");
      await screen.findByText("Stream complete");
      expect(
        fetch.mock.calls.some(([url]) => String(url).includes("/download")),
      ).toBe(false);
    },
  );

  it.each(["LOG_FINALIZING", "LOG_INCOMPLETE"])(
    "shows %s for the exact selected log version",
    async (code) => {
      localStorage.setItem("access_token", "test-token");
      vi.spyOn(axios, "request").mockResolvedValue({
        status: 200,
        data: page([{ ...header, selected_version: selected }]),
        headers: {},
      });
      const fetch = vi
        .fn()
        .mockResolvedValue(
          new Response(JSON.stringify({ code }), { status: 409 }),
        );
      vi.stubGlobal("fetch", fetch);
      mount(<ExecutionArtifactsPanel executionId={10} />);
      fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
      fireEvent.click(screen.getByRole("button", { name: "Preview" }));
      await screen.findByText(
        code === "LOG_FINALIZING"
          ? /Log is still being finalized/
          : /Log is incomplete because delivery did not finish/,
      );
      expect(fetch.mock.calls[0][0]).toContain(
        "/artifacts/42/versions/7/download?version_id=71",
      );
      expect(fetch).toHaveBeenCalledTimes(1);
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

describe("log delivery after execution completion", () => {
  it("keeps an open log stream and accepts its final tail after completion", async () => {
    localStorage.setItem("access_token", "test-token");
    vi.spyOn(axios, "request").mockResolvedValue({
      status: 200,
      data: page([{ ...header, selected_version: selected }]),
      headers: {},
    });
    let source!: ReadableStreamDefaultController<Uint8Array>;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        source = controller;
      },
    });
    const fetch = vi.fn().mockResolvedValue(new Response(body));
    vi.stubGlobal("fetch", fetch);
    function View() {
      const [running, setRunning] = useState(true);
      return (
        <>
          <button onClick={() => setRunning(false)}>Complete action</button>
          <ExecutionArtifactsPanel executionId={10} isRunning={running} />
        </>
      );
    }
    mount(<View />);
    fireEvent.click(await screen.findByRole("button", { name: /stdout/ }));
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    await act(async () => {
      source.enqueue(
        new TextEncoder().encode("id: 6\nevent: content\ndata: before\n\n"),
      );
    });
    await screen.findByText("before");
    fireEvent.click(screen.getByRole("button", { name: "Complete action" }));
    expect(fetch.mock.calls[0][1].signal.aborted).toBe(false);
    await act(async () => {
      source.enqueue(
        new TextEncoder().encode(
          "id: 12\nevent: append\ndata:  after\n\nid: 12\nevent: done\ndata: verified\n\n",
        ),
      );
    });
    await screen.findByText("Stream complete");
    expect(screen.getByText("before after")).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledOnce();
  });
});
