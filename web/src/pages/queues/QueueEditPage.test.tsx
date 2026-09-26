import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import QueueEditPage from "./QueueEditPage";

let queueIsAdhoc = true;
let canUpdate = false;

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({
    user: {
      effective_permissions: canUpdate
        ? [{ resource: "queues", actions: ["update"] }]
        : [],
    },
  }),
}));

vi.mock("@/hooks/useQueues", () => ({
  useQueue: () => ({
    data: {
      data: {
        ref: "ops.inbox",
        is_adhoc: queueIsAdhoc,
      },
    },
    isLoading: false,
    error: null,
  }),
}));

vi.mock("@/components/queues/QueueForm", () => ({
  default: () => <div>Queue edit form</div>,
}));

function renderPage() {
  render(
    <MemoryRouter initialEntries={["/queues/ops.inbox/edit"]}>
      <Routes>
        <Route path="/queues/:ref/edit" element={<QueueEditPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("QueueEditPage", () => {
  beforeEach(() => {
    queueIsAdhoc = true;
    canUpdate = false;
  });

  it("does not render the edit form without queues:update", () => {
    renderPage();

    expect(screen.queryByText("Queue edit form")).toBeNull();
    expect(screen.getByText("queues:update")).toBeInTheDocument();
  });

  it("does not render the edit form for a pack-managed queue", () => {
    queueIsAdhoc = false;
    canUpdate = true;

    renderPage();

    expect(screen.queryByText("Queue edit form")).toBeNull();
    expect(
      screen.getByText(/pack-managed and cannot be edited/i),
    ).toBeInTheDocument();
  });

  it("renders the full edit flow for an authorized API-managed queue", () => {
    canUpdate = true;

    renderPage();

    expect(screen.getByText("Queue edit form")).toBeInTheDocument();
  });
});
