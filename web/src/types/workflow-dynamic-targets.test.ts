import { describe, expect, it } from "vitest";
import yaml from "js-yaml";
import {
  builderStateToDefinition,
  builderStateToGraph,
  definitionToBuilderState,
  locateWorkflowTasks,
  updateWorkflowTask,
  validateAllowedActionRefs,
  validateWorkflow,
  retainUnrenderedTaskInputs,
  type WorkflowYamlDefinition,
} from "./workflow";

function historicalDefinition(): WorkflowYamlDefinition {
  return {
    ref: "example.flow",
    label: "Historical workflow",
    version: "1.0",
    parameters: {},
    tasks: {
      "group/name~": {
        type: "parallel",
        worker_selector: { zone: "retained" },
        tasks: {
          "choose/target": {
            action: "{{ parameters.target }}",
            allowed_action_refs: ["example.one", "foreign.two"],
            params: { token: "{{ keystore['pack.token'] }}" },
            permission_set_refs: [
              "{{ parameters.permissions }}",
              "example.extra",
            ],
            with_items: "{{ parameters.items }}",
            batch_size: 4,
            concurrency: 2,
            retry: { count: 2, delay: 0 },
            worker_affinity: { preferred: [] },
            tasks: null,
          },
          cache: {
            action_ref: "{{ item.target }}",
            allowed_action_refs: ["foreign.two"],
            input: { item: "{{ item }}" },
            permission_set_ref: [],
            iterate_cache: {
              owner_type: "pack",
              owner_ref: "example",
              namespace: "inventory",
              page_size: 20,
              require_fresh: false,
            },
            batch_size: 2,
            concurrency: 3,
          },
        },
      },
    },
  };
}

describe("finite workflow target conversion", () => {
  it("never substitutes publisher-current defaults or required fields for retained inputs", () => {
    const state = definitionToBuilderState(
      {
        ref: "example.flow",
        label: "Flow",
        version: "1.0",
        tasks: [
          {
            name: "stored",
            action: "foreign.action",
            allowed_action_refs: ["foreign.action"],
            input: { mode: "keep-explicit", legacy: "retained" },
          },
        ],
      },
      "example",
      "flow",
    );
    const currentPreview = new Map([
      [
        "foreign.action",
        {
          mode: { type: "string", default: "keep-explicit" },
          newly_required: { type: "string", required: true },
        },
      ],
    ]);
    expect(
      builderStateToGraph({ ...state, label: "New label" }, currentPreview)
        .tasks[0].input,
    ).toEqual({ mode: "keep-explicit", legacy: "retained" });
    expect(validateWorkflow(state, currentPreview)).toEqual([]);
    expect(
      retainUnrenderedTaskInputs({
        stored: state.tasks[0].input,
        edited: { mode: "new" },
        renderedFields: ["mode", "newly_required"],
      }),
    ).toEqual({ mode: "new", legacy: "retained" });
    expect(
      retainUnrenderedTaskInputs({
        stored: state.tasks[0].input,
        edited: {},
        renderedFields: ["mode"],
      }),
    ).toEqual({ legacy: "retained" });
  });
  it("keeps escaped named scopes, nested/fanout/cache fields and ceilings through unrelated edits and YAML export", () => {
    const source = historicalDefinition();
    const retainedSource = structuredClone(source);
    let state = definitionToBuilderState(source, "example", "flow");
    const entries = locateWorkflowTasks(state.tasks, state.taskCollectionShape);
    expect(new Set(entries.map(({ task }) => task.id)).size).toBe(
      entries.length,
    );
    expect(entries[1].path).toBe("/tasks/group~1name~0/tasks/choose~1target");
    state = {
      ...state,
      label: "Only the label changed",
      parameters: { target: { type: "string" } },
      tasks: updateWorkflowTask(state.tasks, entries[1].task.id, {
        input: { token: "{{ keystore['pack.other'] }}" },
      }),
    };
    const saved = builderStateToDefinition(state);
    const graph = builderStateToGraph(state);
    expect(saved.tasks).toEqual(graph.tasks);
    expect(saved.tasks).toMatchObject({
      "group/name~": {
        type: "parallel",
        worker_selector: { zone: "retained" },
        tasks: {
          "choose/target": {
            allowed_action_refs: ["example.one", "foreign.two"],
            params: { token: "{{ keystore['pack.other'] }}" },
            permission_set_refs: [
              "{{ parameters.permissions }}",
              "example.extra",
            ],
            with_items: "{{ parameters.items }}",
            retry: { count: 2, delay: 0 },
            batch_size: 4,
            concurrency: 2,
            worker_affinity: { preferred: [] },
          },
          cache: {
            allowed_action_refs: ["foreign.two"],
            permission_set_refs: [],
            iterate_cache: { namespace: "inventory", generation: "active" },
            batch_size: 2,
            concurrency: 3,
          },
        },
      },
    });
    const exported: unknown = yaml.load(yaml.dump(graph));
    expect(exported).toEqual(graph);
    const reopened = definitionToBuilderState(saved, "example", "flow");
    expect(builderStateToGraph(reopened)).toEqual(graph);
    expect(validateWorkflow(reopened)).toEqual([]);
    expect(source).toEqual(retainedSource);
  });

  it.each([
    undefined,
    [],
    null,
    ["example.*"],
    ["{{ parameters.target }}"],
    ["short"],
    [42],
    Array.from({ length: 257 }, () => "example.one"),
  ])("reports a located invalid ceiling for %j", (allowed) => {
    expect(
      validateAllowedActionRefs(
        { action: "{{ parameters.target }}", allowed_action_refs: allowed },
        "/tasks/group~1name/tasks/0",
      )[0],
    ).toContain("/tasks/group~1name/tasks/0/allowed_action_refs");
  });

  it("requires a literal selector to belong to its ceiling and does not infer admission from discovery", () => {
    expect(
      validateAllowedActionRefs(
        { action: "example.two", allowed_action_refs: ["example.one"] },
        "/tasks/0",
      ),
    ).toEqual([
      "/tasks/0/action: literal action must belong to allowed_action_refs",
    ]);
    // A public or private relationship cannot be proven client-side. Unknown
    // discovery results do not remove historical or manually entered targets.
    expect(
      validateAllowedActionRefs(
        {
          action: "{{ parameters.target }}",
          allowed_action_refs: ["foreign.private"],
        },
        "/tasks/0",
      ),
    ).toEqual([]);
    const state = definitionToBuilderState(
      {
        ...historicalDefinition(),
        tasks: [
          {
            name: "choose",
            action: "{{ parameters.target }}",
            allowed_action_refs: [],
          },
        ],
      },
      "example",
      "flow",
    );
    expect(builderStateToGraph(state).tasks[0].allowed_action_refs).toEqual([]);
    expect(validateWorkflow(state).join("\n")).toContain(
      "/tasks/0/allowed_action_refs",
    );
  });

  it("denies ambiguous numeric array names instead of changing their authored identity", () => {
    const state = definitionToBuilderState(
      {
        ...historicalDefinition(),
        tasks: [
          { name: "first", action: "example.one" },
          { name: "0", action: "example.one" },
        ],
      },
      "example",
      "flow",
    );
    expect(validateWorkflow(state)).toContain(
      "/tasks/0: ambiguous numeric task name/index",
    );
  });
});
