import { describe, expect, it } from "vitest";
import {
  builderStateToGraph,
  definitionToBuilderState,
  validateWorkflow,
  type WorkflowBuilderState,
  type WorkflowTask,
  type WorkflowYamlDefinition,
} from "./workflow";

function task(overrides: Partial<WorkflowTask> = {}): WorkflowTask {
  return {
    id: "task-1",
    name: "process_cache",
    action: "core.echo",
    input: {},
    position: { x: 0, y: 0 },
    ...overrides,
  };
}

function state(workflowTask: WorkflowTask): WorkflowBuilderState {
  return {
    name: "cache_workflow",
    label: "Cache workflow",
    description: "",
    version: "1.0.0",
    packRef: "core",
    referenceVisibility: "public",
    referenceAllowedPackRefs: [],
    parameters: {},
    output: {},
    outputMap: {},
    vars: {},
    tasks: [workflowTask],
    tags: [],
    cancellationPolicy: "allow_finish",
  };
}

describe("iterate_cache workflow authoring", () => {
  it("round-trips the typed cache iteration block", () => {
    const workflowState = state(
      task({
        iterate_cache: {
          owner_type: "pack",
          owner_ref: "core",
          namespace: "inventory",
          generation: "{{ parameters.generation }}",
          page_size: 250,
          require_fresh: true,
        },
        batch_size: 25,
        concurrency: 4,
      }),
    );

    const graph = builderStateToGraph(workflowState);
    expect(graph.tasks[0].iterate_cache).toEqual(
      workflowState.tasks[0].iterate_cache,
    );
    expect(graph.tasks[0].batch_size).toBe(25);
    expect(graph.tasks[0].concurrency).toBe(4);

    const definition: WorkflowYamlDefinition = {
      ref: "core.cache_workflow",
      label: "Cache workflow",
      version: "1.0.0",
      tasks: graph.tasks,
    };
    const roundTrippedTask = definitionToBuilderState(
      definition,
      "core",
      "cache_workflow",
    ).tasks[0];
    expect(roundTrippedTask.iterate_cache).toEqual(
      workflowState.tasks[0].iterate_cache,
    );
    expect(roundTrippedTask.batch_size).toBe(25);
  });

  it("applies parser defaults when loading an omitted cache generation config", () => {
    const definition: WorkflowYamlDefinition = {
      ref: "core.cache_workflow",
      label: "Cache workflow",
      version: "1.0.0",
      tasks: [
        {
          name: "process_cache",
          action: "core.echo",
          iterate_cache: {
            owner_type: "system",
            namespace: "inventory",
          },
        },
      ],
    };

    expect(
      definitionToBuilderState(definition, "core", "cache_workflow").tasks[0]
        .iterate_cache,
    ).toEqual({
      owner_type: "system",
      owner_ref: undefined,
      namespace: "inventory",
      generation: "active",
      page_size: 100,
      require_fresh: false,
    });
  });

  it("validates selector fields, page bounds, and iteration exclusivity", () => {
    const errors = validateWorkflow(
      state(
        task({
          with_items: "{{ parameters.items }}",
          batch_size: 10,
          iterate_cache: {
            owner_type: "pack",
            namespace: "",
            generation: "latest",
            page_size: 1001,
            require_fresh: false,
          },
        }),
      ),
    );

    expect(errors).toContain(
      'Task "process_cache" cannot define both with_items and iterate_cache',
    );
    expect(errors).toContain(
      'Task "process_cache" cache owner reference is required for pack ownership',
    );
    expect(errors).toContain(
      'Task "process_cache" cache namespace is required',
    );
    expect(errors).toContain(
      'Task "process_cache" cache generation must be active, a positive generation ID, or an expression',
    );
    expect(errors).toContain(
      'Task "process_cache" cache page size must be between 1 and 1000',
    );
    expect(errors).not.toContain(
      'Task "process_cache" batch size requires with_items or iterate_cache',
    );
  });

  it("allows batch size and concurrency for cache iteration independently of page size", () => {
    const errors = validateWorkflow(
      state(
        task({
          iterate_cache: {
            owner_type: "system",
            namespace: "inventory",
            generation: "active",
            page_size: 100,
            require_fresh: false,
          },
          batch_size: 1,
          concurrency: 2,
        }),
      ),
    );

    expect(errors).not.toContain(
      'Task "process_cache" concurrency requires with_items or iterate_cache',
    );
    expect(errors).not.toContain(
      'Task "process_cache" batch size requires with_items or iterate_cache',
    );
  });
});
