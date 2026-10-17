import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkflowTask, WorkflowYamlDefinition } from "@/types/workflow";
import WorkflowBuilderPage from "./WorkflowBuilderPage";

const api = vi.hoisted(() => {
  const definition: WorkflowYamlDefinition = {
    ref: "example.flow",
    label: "Historical flow",
    version: "1.0",
    tasks: [],
  };
  return {
    definition,
    save: vi.fn<(request: unknown) => Promise<unknown>>(),
    update: vi.fn<(request: unknown) => Promise<unknown>>(),
    execute: vi.fn<(request: unknown) => Promise<{ data: { id: number } }>>(),
    action: vi.fn<(reference: string) => unknown>(),
    discovery: vi.fn(),
  };
});

vi.mock("@/hooks/usePacks", () => ({
  usePacks: () => ({
    data: { items: [{ id: 1, ref: "example", label: "Example" }] },
  }),
}));
vi.mock("@/hooks/useActions", () => ({
  useAction: (reference: string) => {
    api.action(reference);
    return {
      data:
        reference === "example.flow"
          ? {
              data: {
                reference_visibility: "public",
                reference_allowed_pack_refs: [],
              },
            }
          : undefined,
      isLoading: false,
    };
  },
  useActions: (options: unknown) => {
    api.discovery(options);
    return {
      data: {
        items: [
          {
            id: 2,
            ref: "foreign.public",
            label: "Public suggestion",
            pack_ref: "foreign",
          },
        ],
      },
      isLoading: false,
    };
  },
}));
vi.mock("@/hooks/useWorkflows", () => ({
  useWorkflow: () => ({
    data: {
      data: {
        ref: "example.flow",
        pack_ref: "example",
        label: "Historical flow",
        version: "1.0",
        tags: [],
        definition: api.definition,
      },
    },
    isLoading: false,
  }),
  useSaveWorkflowFile: () => ({ mutateAsync: api.save, isPending: false }),
  useUpdateWorkflowFile: () => ({ mutateAsync: api.update, isPending: false }),
}));
vi.mock("@/hooks/useExecutions", () => ({
  useRequestExecution: () => ({ mutateAsync: api.execute, isPending: false }),
}));
// The chart is not the authoring boundary under test. Keep the real page,
// nested task selector, inspector, validation, converters and save/run handlers.
vi.mock("@/components/workflows/WorkflowCanvas", () => ({
  default: ({
    tasks,
    onSelectTask,
  }: {
    tasks: WorkflowTask[];
    onSelectTask: (id: string) => void;
  }) => (
    <div>
      {tasks.map((task) => (
        <button key={task.id} onClick={() => onSelectTask(task.id)}>
          Inspect {task.name}
        </button>
      ))}
    </div>
  ),
}));
vi.mock("@/components/workflows/ActionPalette", () => ({
  default: () => <div>Server action discovery</div>,
}));

function definition(
  allowed: string[] | undefined = ["foreign.historical"],
): WorkflowYamlDefinition {
  return {
    ref: "example.flow",
    label: "Historical flow",
    version: "1.0",
    tasks: {
      "group/name~": {
        type: "parallel",
        worker_selector: { zone: "original" },
        tasks: {
          "choose/target": {
            action: "{{ parameters.target }}",
            allowed_action_refs: allowed,
            permission_set_refs: "{{ parameters.reduced_refs }}",
            input: { value: "{{ item }}" },
            with_items: "{{ parameters.items }}",
            batch_size: 3,
            concurrency: 2,
            retry: { count: 2, delay: 0 },
          },
        },
      },
    },
  };
}

const clients: QueryClient[] = [];
function showBuilder() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  clients.push(client);
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/actions/workflows/example.flow/edit"]}>
        <Routes>
          <Route
            path="/actions/workflows/:ref/edit"
            element={<WorkflowBuilderPage />}
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function inspectNestedTask() {
  const option = screen.getByRole("option", { name: /choose\/target/ });
  fireEvent.change(screen.getByLabelText("Task to edit"), {
    target: { value: option.getAttribute("value") },
  });
}

describe("workflow builder finite target authoring", () => {
  beforeEach(() => {
    const values = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
      removeItem: (key: string) => {
        values.delete(key);
      },
      clear: () => values.clear(),
      key: (index: number) => [...values.keys()][index] ?? null,
      get length() {
        return values.size;
      },
    } satisfies Storage);
    api.definition = definition();
    api.save.mockReset().mockResolvedValue({});
    api.update.mockReset().mockResolvedValue({});
    api.execute.mockReset().mockResolvedValue({ data: { id: 42 } });
    api.action.mockClear();
    api.discovery.mockClear();
    vi.spyOn(window, "open").mockReturnValue(null);
  });
  afterEach(() => {
    clients.splice(0).forEach((client) => client.clear());
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("edits nested historical targets and unrelated metadata, then preserves them in save, YAML export and save-before-run", async () => {
    const view = showBuilder();
    inspectNestedTask();
    expect(screen.getByLabelText("Action selector")).toHaveValue(
      "{{ parameters.target }}",
    );
    expect(screen.getByLabelText("Allowed action target 1")).toHaveValue(
      "foreign.historical",
    );
    expect(api.action).not.toHaveBeenCalledWith("{{ parameters.target }}");
    expect(api.discovery).toHaveBeenCalledWith(
      expect.objectContaining({ referencingPackRef: "example" }),
    );
    fireEvent.change(screen.getByLabelText("Allowed action target 1"), {
      target: { value: "foreign.manually_entered" },
    });
    fireEvent.change(screen.getByLabelText("Stored input value"), {
      target: { value: "{{ parameters.updated_item }}" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Workflow Options/ }));
    fireEvent.change(screen.getByDisplayValue("Historical flow"), {
      target: { value: "Only a label edit" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
    const expected = expect.objectContaining({
      data: expect.objectContaining({
        label: "Only a label edit",
        definition: expect.objectContaining({
          tasks: {
            "group/name~": expect.objectContaining({
              type: "parallel",
              worker_selector: { zone: "original" },
              tasks: {
                "choose/target": expect.objectContaining({
                  action: "{{ parameters.target }}",
                  allowed_action_refs: ["foreign.manually_entered"],
                  permission_set_refs: "{{ parameters.reduced_refs }}",
                  input: { value: "{{ parameters.updated_item }}" },
                  with_items: "{{ parameters.items }}",
                  batch_size: 3,
                  concurrency: 2,
                  retry: { count: 2, delay: 0 },
                }),
              },
            }),
          },
        }),
      }),
    });
    expect(api.update).toHaveBeenLastCalledWith(expected);
    fireEvent.click(screen.getByRole("button", { name: "Raw YAML" }));
    expect(view.container.textContent).toContain("allowed_action_refs:");
    expect(view.container.textContent).toContain("foreign.manually_entered");
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await waitFor(() =>
      expect(api.execute).toHaveBeenCalledWith({
        actionRef: "example.flow",
        parameters: {},
      }),
    );
    expect(api.update).toHaveBeenLastCalledWith(expected);
    expect(window.open).toHaveBeenCalledWith("/executions/42", "_blank");
  });

  it("repairs a missing historical ceiling through real fields and blocks empty, wildcard, template and nonmember literal targets with located errors", async () => {
    api.definition = definition(undefined);
    // Explicitly remove the pre-admission field, rather than normalizing it away.
    const source = api.definition.tasks;
    if (Array.isArray(source)) throw new Error("Expected named fixture");
    const children = source["group/name~"].tasks;
    if (!children || Array.isArray(children))
      throw new Error("Expected named children");
    delete children["choose/target"].allowed_action_refs;
    showBuilder();
    inspectNestedTask();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "/tasks/group~1name~0/tasks/choose~1target/allowed_action_refs",
    );
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    expect(api.update).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Add action target" }));
    for (const invalid of ["", "foreign.*", "{{ parameters.escape }}"]) {
      fireEvent.change(screen.getByLabelText("Allowed action target 1"), {
        target: { value: invalid },
      });
      expect(screen.getByRole("alert")).toHaveTextContent(
        "/allowed_action_refs/0",
      );
    }
    fireEvent.change(screen.getByLabelText("Allowed action target 1"), {
      target: { value: "foreign.public" },
    });
    fireEvent.change(screen.getByLabelText("Action selector"), {
      target: { value: "foreign.other" },
    });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "/action: literal action must belong",
    );
    fireEvent.change(screen.getByLabelText("Action selector"), {
      target: { value: "{{ parameters.target }}" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(1));
  });

  it("keeps foreign private targets subject to server admission and surfaces rejection without erasing the entered ceiling or running", async () => {
    api.definition = definition(["foreign.private"]);
    api.update.mockRejectedValue({
      status: 403,
      body: {
        message:
          "api/workflows /tasks/group~1name~0/tasks/choose~1target/allowed_action_refs: foreign action is private",
      },
    });
    showBuilder();
    inspectNestedTask();
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await waitFor(() =>
      expect(screen.getByText(/foreign action is private/)).toBeInTheDocument(),
    );
    expect(api.update).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Allowed action target 1")).toHaveValue(
      "foreign.private",
    );
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await waitFor(() => expect(api.update).toHaveBeenCalledTimes(2));
    expect(api.execute).not.toHaveBeenCalled();
  });
});
