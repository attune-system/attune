import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActionReferenceVisibility } from "@/api";
import type { TriggerResponse } from "@/api";
import TriggerForm from "./TriggerForm";
import formRequests from "../../test/fixtures/form-requests.json";

const mocks = vi.hoisted(() => ({
  update: vi.fn().mockResolvedValue(undefined),
  packs: { items: [{ id: 1, ref: "demo", label: "Demo" }] },
}));
vi.mock("@/hooks/usePacks", () => ({
  usePacks: () => ({ data: mocks.packs }),
}));
vi.mock("@/hooks/useTriggers", () => ({
  useCreateTrigger: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useUpdateTrigger: () => ({ mutateAsync: mocks.update, isPending: false }),
}));

afterEach(() => {
  cleanup();
  mocks.update.mockClear();
});

const trigger: TriggerResponse = {
  id: 1,
  ref: "demo.event",
  pack: 1,
  pack_ref: "demo",
  label: "Event",
  description: "",
  enabled: true,
  is_adhoc: true,
  webhook_enabled: false,
  reference_visibility: ActionReferenceVisibility.PUBLIC,
  created: "2026-10-13",
  updated: "2026-10-13",
  param_schema: {
    target: { type: "demo.address", required: true },
    mode: { type: ["string", "null"], default: null, required: false },
  },
  out_schema: {},
  event_config_schema: { token: { type: "string", secret: true } },
};

function showForm() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <TriggerForm initialData={trigger} isEditing />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("trigger section schema authoring", () => {
  it("omits create-only identity fields from updates", async () => {
    showForm();
    await screen.findByDisplayValue("Event");
    fireEvent.click(screen.getByRole("button", { name: "Update Trigger" }));
    await waitFor(() => expect(mocks.update).toHaveBeenCalledOnce());
    const request = mocks.update.mock.calls[0][0];
    expect(request.ref).toBe(trigger.ref);
    expect(request.data).not.toHaveProperty("pack_ref");
    expect(request.data).not.toHaveProperty("ref");
    expect(JSON.parse(JSON.stringify(request.data))).toEqual(
      formRequests.triggerUpdate,
    );
  });

  it("preserves authored references and explicit empty schemas using tagged patches", async () => {
    showForm();
    await screen.findByDisplayValue("Event");
    fireEvent.click(screen.getByRole("button", { name: "Update Trigger" }));
    await waitFor(() => expect(mocks.update).toHaveBeenCalledOnce());
    expect(mocks.update).toHaveBeenCalledWith({
      ref: "demo.event",
      data: expect.objectContaining({
        param_schema: { op: "set", value: trigger.param_schema },
        out_schema: { op: "set", value: {} },
        event_config_schema: { op: "set", value: trigger.event_config_schema },
        description: { op: "clear" },
      }),
    });
  });

  it("clears event configuration only when explicitly requested", async () => {
    showForm();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Remove event configuration schema",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Update Trigger" }));
    await waitFor(() => expect(mocks.update).toHaveBeenCalledOnce());
    expect(mocks.update).toHaveBeenCalledWith({
      ref: "demo.event",
      data: expect.objectContaining({
        event_config_schema: { op: "clear" },
        out_schema: { op: "set", value: {} },
      }),
    });
  });
});
