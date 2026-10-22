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
import { RulesService, SchemaTypesService } from "@/api";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import type { RuleResponse } from "@/api";
import RuleForm from "./RuleForm";
import formRequests from "../../test/fixtures/form-requests.json";

const { triggerFixture } = vi.hoisted(() => {
  const param_schema: Record<string, unknown> = {};
  return { triggerFixture: { ref: "demo.event", param_schema } };
});

vi.mock("@/hooks/usePacks", () => ({
  usePacks: () => ({
    data: { items: [{ id: 1, ref: "demo", label: "Demo", version: "1.0.0" }] },
  }),
}));
vi.mock("@/hooks/useTriggers", () => ({
  useTriggers: () => ({
    data: { items: [{ id: 2, ref: "demo.event", label: "Event" }] },
  }),
  useTrigger: () => ({
    data: { data: triggerFixture },
  }),
}));
vi.mock("@/hooks/useActions", () => ({
  useActions: () => ({
    data: { items: [{ id: 3, ref: "demo.echo", label: "Echo" }] },
  }),
  useAction: () => ({ data: { data: { ref: "demo.echo", param_schema: {} } } }),
}));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  triggerFixture.param_schema = {};
});

const rule: RuleResponse = {
  id: 4,
  ref: "demo.notify",
  pack: 1,
  pack_ref: "demo",
  label: "Notify",
  description: "Send a notification",
  trigger: 2,
  trigger_ref: "demo.event",
  action: 3,
  action_ref: "demo.echo",
  conditions: {},
  trigger_params: { interval: 30 },
  action_params: { message: "{{ event.payload.message }}" },
  sensor_worker_selector: { region: "west" },
  sensor_worker_tolerations: [],
  sensor_worker_affinity: {},
  permission_set_refs: null,
  trace_tag_template: null,
  enabled: true,
  is_adhoc: true,
  created: "2026-10-14T00:00:00Z",
  updated: "2026-10-14T00:00:00Z",
};

function showForm(initialRule?: RuleResponse) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <RuleForm rule={initialRule} onSuccess={() => {}} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("rule request contracts", () => {
  it("renders trigger parameter enums as selects and submits the chosen value", async () => {
    triggerFixture.param_schema = {
      mode: { type: "string", enum: ["interval", "cron"], required: true },
    };
    vi.spyOn(SchemaTypesService, "previewSchema").mockReturnValue(
      new CancelablePromise<
        Awaited<ReturnType<typeof SchemaTypesService.previewSchema>>
      >((resolve) =>
        resolve({
          data: {
            semantic_digest: "trigger-enum-preview",
            contract: {
              annotated_schema: {
                type: "object",
                required: ["mode"],
                properties: {
                  mode: { type: "string", enum: ["interval", "cron"] },
                },
              },
            },
          },
        }),
      ),
    );
    const update = vi.spyOn(RulesService, "updateRule").mockResolvedValue({
      data: rule,
    });
    showForm({ ...rule, trigger_params: { mode: "interval" } });
    const select = await screen.findByRole("combobox", { name: "/mode" });
    expect(select).toHaveValue("0");
    fireEvent.change(select, { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: "Update Rule" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0][0].requestBody.trigger_params).toEqual({
      mode: "cron",
    });
  });
  it("updates a rule without create-only identity fields in the API body", async () => {
    const update = vi.spyOn(RulesService, "updateRule").mockResolvedValue({
      data: rule,
    });
    showForm(rule);
    fireEvent.change(screen.getByLabelText(/Label/), {
      target: { value: "Updated notification" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update Rule" }));

    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    const request = update.mock.calls[0][0];
    expect(request.ref).toBe(rule.ref);
    expect(request.requestBody).not.toHaveProperty("pack_ref");
    expect(request.requestBody).not.toHaveProperty("ref");
    expect(JSON.parse(JSON.stringify(request.requestBody))).toEqual(
      formRequests.ruleUpdate,
    );
  });

  it("keeps pack_ref and ref in create requests", async () => {
    const create = vi.spyOn(RulesService, "createRule").mockResolvedValue({
      data: rule,
    });
    showForm();
    fireEvent.click(screen.getByText("Select a pack..."));
    fireEvent.mouseDown(screen.getByRole("option", { name: "Demo (1.0.0)" }));
    fireEvent.change(screen.getByLabelText(/Label/), {
      target: { value: "Notify" },
    });
    fireEvent.change(screen.getByLabelText(/Reference/), {
      target: { value: "notify" },
    });
    fireEvent.click(screen.getByText("Select a trigger..."));
    fireEvent.mouseDown(
      screen.getByRole("option", { name: "demo.event - Event" }),
    );
    fireEvent.click(screen.getByText("Select an action..."));
    fireEvent.mouseDown(
      screen.getByRole("option", { name: "demo.echo - Echo" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Create Rule" }));

    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0][0].requestBody).toEqual({
      pack_ref: "demo",
      ref: "demo.notify",
      label: "Notify",
      trigger_ref: "demo.event",
      action_ref: "demo.echo",
      enabled: true,
      trace_tag_template: null,
      sensor_worker_selector: {},
      sensor_worker_tolerations: [],
      sensor_worker_affinity: {},
    });
  });
});
