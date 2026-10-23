import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  WorkQueueBatchMode,
  WorkQueueUpdateStrategy,
  ReferenceVisibility,
  type WorkQueueResponse,
} from "@/api/queues";
import QueueForm from "./QueueForm";

const update = vi.hoisted(() => ({ isPending: false, mutateAsync: vi.fn() }));
vi.mock("@/hooks/useQueues", () => ({
  useCreateQueue: () => update,
  useUpdateQueue: () => update,
}));
vi.mock("@/hooks/usePacks", () => ({
  usePacks: () => ({ data: { items: [] } }),
}));
vi.mock("@/hooks/useActions", () => ({
  useActions: () => ({ data: { items: [] } }),
  useAction: () => ({
    data: { data: { param_schema: {} } },
    isFetching: false,
  }),
}));

function queue(separate?: boolean): WorkQueueResponse {
  return {
    id: 1,
    ref: "adhoc.inbox",
    label: "Inbox",
    dispatch_action_ref: "ops.process",
    enabled: true,
    accepting_new_items: true,
    is_adhoc: true,
    default_priority: 0,
    allow_pending_update: false,
    batch_mode: WorkQueueBatchMode.BATCH,
    update_strategy: WorkQueueUpdateStrategy.IMMUTABLE,
    reference_visibility: ReferenceVisibility.PUBLIC,
    reference_allowed_pack_refs: [],
    created: "2026-10-15T00:00:00Z",
    updated: "2026-10-15T00:00:00Z",
    item_schema: {},
    action_params: {},
    config: {
      dispatch: {
        ...(separate === undefined ? {} : { separate_requesters: separate }),
        batch_size: { source: "literal", value: 16 },
      },
    },
  };
}

describe("QueueForm requester grouping", () => {
  beforeEach(() => {
    update.mutateAsync.mockReset();
    update.mutateAsync.mockResolvedValue({});
  });

  it.each([undefined, false, true])(
    "shows the effective grouping without opening advanced configuration: %s",
    (separate) => {
      render(
        <MemoryRouter>
          <QueueForm initialData={queue(separate)} isEditing />
        </MemoryRouter>,
      );
      expect(
        screen.getByRole("combobox", { name: "Requester grouping" }),
      ).toHaveValue(separate ? "separate" : "aggregate");
      expect(
        screen.getByRole("button", { name: /Advanced queue config/ }),
      ).toHaveAttribute("aria-expanded", "false");
    },
  );

  it.each([false, true])(
    "saves an explicit grouping choice: %s",
    async (separate) => {
      render(
        <MemoryRouter>
          <QueueForm initialData={queue(!separate)} isEditing />
        </MemoryRouter>,
      );
      fireEvent.change(
        screen.getByRole("combobox", { name: "Requester grouping" }),
        { target: { value: separate ? "separate" : "aggregate" } },
      );
      fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
      await waitFor(() => expect(update.mutateAsync).toHaveBeenCalled());
      expect(update.mutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          data: expect.objectContaining({
            config: expect.objectContaining({
              dispatch: expect.objectContaining({
                separate_requesters: separate,
                batch_size: { source: "literal", value: 16 },
              }),
            }),
          }),
        }),
      );
    },
  );
});
