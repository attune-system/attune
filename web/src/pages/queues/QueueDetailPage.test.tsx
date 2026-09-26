import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ReferenceVisibility,
  WorkQueueBatchMode,
  WorkQueueUpdateStrategy,
} from "@/api/queues";
import { QueueDetailPage } from "./QueueDetailPage";

let currentUser: {
  effective_permissions: Array<{ resource: string; actions: string[] }>;
};
let queueIsAdhoc = true;
let actionResult: { data?: { data: Record<string, unknown> }; error?: Error };

const updateQueue = {
  isPending: false,
  variables: undefined,
  mutateAsync: vi.fn(),
};

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ user: currentUser }),
}));

vi.mock("@/hooks/useActions", () => ({
  useAction: () => actionResult,
}));

vi.mock("@/hooks/useQueueStream", () => ({
  useQueueStream: () => ({ isConnected: false }),
}));

vi.mock("@/hooks/useQueues", () => ({
  useQueue: () => ({
    data: {
      data: {
        id: 1,
        ref: "ops.inbox",
        pack_ref: "ops",
        is_adhoc: queueIsAdhoc,
        label: "Operations inbox",
        description: "Incoming operational work",
        enabled: true,
        accepting_new_items: true,
        dispatch_action_ref: "ops.process_item",
        trace_tag_template: null,
        reference_visibility: ReferenceVisibility.PUBLIC,
        reference_allowed_pack_refs: [],
        created: "2026-09-01T00:00:00Z",
        updated: "2026-09-01T00:00:00Z",
        default_priority: 0,
        allow_pending_update: false,
        update_strategy: WorkQueueUpdateStrategy.IMMUTABLE,
        batch_mode: WorkQueueBatchMode.SINGLE,
        item_schema: {},
        action_params: {},
        config: {},
      },
    },
    isLoading: false,
    error: null,
  }),
  useQueueItems: () => ({
    data: {
      items: [],
      pagination: {
        page: 1,
        page_size: 20,
        total_items: 0,
        total_pages: 0,
        has_previous: false,
        has_next: false,
      },
    },
    isLoading: false,
    isFetching: false,
    error: null,
  }),
  useUpdateQueue: () => updateQueue,
  useDeleteQueue: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useDeleteQueueItem: () => ({ isPending: false, mutateAsync: vi.fn() }),
  usePreviewQueueItemsBySelector: () => ({
    isPending: false,
    mutateAsync: vi.fn(),
  }),
  useApplyQueueItemsBySelector: () => ({
    isPending: false,
    mutateAsync: vi.fn(),
  }),
}));

function renderPage() {
  render(
    <MemoryRouter initialEntries={["/queues/ops.inbox"]}>
      <Routes>
        <Route path="/queues/:ref" element={<QueueDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("QueueDetailPage", () => {
  beforeEach(() => {
    currentUser = { effective_permissions: [] };
    queueIsAdhoc = true;
    actionResult = {};
    updateQueue.mutateAsync.mockReset();
  });

  it("shows the dispatch action ref, metadata, and detail link when readable", () => {
    actionResult = {
      data: {
        data: {
          ref: "ops.process_item",
          label: "Process inbox item",
          description: "Routes one inbox item to the operations workflow.",
          param_schema: {},
        },
      },
    };

    renderPage();

    expect(screen.getByText("ops.process_item")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Process inbox item" }),
    ).toHaveAttribute("href", "/actions/ops.process_item");
    expect(
      screen.getByText("Routes one inbox item to the operations workflow."),
    ).toBeInTheDocument();
  });

  it("keeps queue detail usable when the action lookup fails", () => {
    actionResult = { error: new Error("Forbidden") };

    renderPage();

    expect(screen.getByText("ops.process_item")).toBeInTheDocument();
    expect(
      screen.getByText("Action details are not available."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /process item/i })).toBeNull();
  });

  it("shows edit only for API-managed queues with queues:update", () => {
    currentUser = {
      effective_permissions: [{ resource: "queues", actions: ["update"] }],
    };

    renderPage();

    expect(screen.getByRole("link", { name: "Edit Queue" })).toHaveAttribute(
      "href",
      "/queues/ops.inbox/edit",
    );
  });

  it("keeps operational controls enabled for authorized pack-managed queues", () => {
    currentUser = {
      effective_permissions: [{ resource: "queues", actions: ["update"] }],
    };
    queueIsAdhoc = false;

    renderPage();

    expect(screen.queryByRole("link", { name: "Edit Queue" })).toBeNull();
    expect(
      screen.getByRole("switch", { name: "Processing enabled" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("switch", { name: "Accepting items" }),
    ).toBeEnabled();
  });

  it("hides edit and disables operational controls without queues:update", () => {
    renderPage();

    expect(screen.queryByRole("link", { name: "Edit Queue" })).toBeNull();
    expect(
      screen.getByRole("switch", { name: "Processing enabled" }),
    ).toBeDisabled();
  });
});
