import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { type PackResponse } from "@/api";
import PackForm from "./PackForm";

const update = vi.fn();
vi.mock("@/hooks/usePacks", () => ({
  useCreatePack: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useUpdatePack: () => ({ mutateAsync: update, isPending: false }),
}));

const pack = {
  id: 1,
  ref: "test",
  label: "Fixture",
  version: "1.0.0",
  conf_schema: { token: { type: "string", secret: true } },
  config: { token: { $attune_secret: true, redacted: true }, extra: "keep-me" },
  config_schema_binding: {
    annotated_schema: {
      type: "object",
      properties: { token: { type: "string", secret: true } },
    },
  },
  meta: {},
  tags: [],
  runtime_deps: [],
  dependencies: [],
  is_standard: false,
  worker_selector: {},
  worker_tolerations: [],
  worker_affinity: {},
  created: "2026-10-13T00:00:00Z",
  updated: "2026-10-13T00:00:00Z",
} satisfies PackResponse;

beforeEach(() => update.mockReset());

describe("pack configuration editing", () => {
  it("resets a secret field without sending its redaction or plaintext", async () => {
    render(
      <MemoryRouter>
        <PackForm pack={pack} onSuccess={vi.fn()} />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByLabelText("Reset token to release default"));
    fireEvent.submit(screen.getByLabelText(/^Label/).closest("form")!);
    await waitFor(() => expect(update).toHaveBeenCalled());
    expect(update.mock.calls[0][0].data.config_reset_fields).toEqual(["token"]);
    expect(update.mock.calls[0][0].data).not.toHaveProperty("config");
  });

  it("does not resubmit redacted configuration or rebind an unchanged schema for metadata edits", async () => {
    render(
      <MemoryRouter>
        <PackForm pack={pack} onSuccess={vi.fn()} />
      </MemoryRouter>,
    );
    fireEvent.change(screen.getByLabelText(/^Label/), {
      target: { value: "Updated fixture" },
    });
    fireEvent.submit(screen.getByLabelText(/^Label/).closest("form")!);
    await waitFor(() => expect(update).toHaveBeenCalled());
    expect(update.mock.calls[0][0].data).not.toHaveProperty("config");
    expect(update.mock.calls[0][0].data).not.toHaveProperty("conf_schema");
  });
});
