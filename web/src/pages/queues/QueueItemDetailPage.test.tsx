import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import QueueItemDetailPage from "./QueueItemDetailPage";

vi.mock("@/hooks/useQueues", () => ({
  useQueueItem: () => ({
    isLoading: false,
    data: {
      data: {
        id: 1,
        queue_ref: "demo.queue",
        status: "queued",
        item_key: "owned-fixture",
        attempt_count: 0,
        updated: "2026-10-14T00:00:00Z",
        metadata: {},
        payload: {
          contact: {
            label: "Original type label",
            credential: "private-queue-fixture",
          },
        },
        schema_binding: {
          contract: {
            annotated_schema: {
              type: "object",
              properties: { contact: { $ref: "#/$defs/contact" } },
              $defs: {
                contact: {
                  type: "object",
                  properties: {
                    label: {
                      type: "string",
                      description: "Retained named contact",
                    },
                    credential: { type: "string", secret: true },
                  },
                },
              },
            },
          },
        },
      },
    },
  }),
}));

describe("queue item stored schema display", () => {
  it("uses the admission contract for nested named values and secret masking", () => {
    render(
      <MemoryRouter initialEntries={["/queues/demo.queue/items/1"]}>
        <Routes>
          <Route
            path="/queues/:ref/items/:itemId"
            element={<QueueItemDetailPage />}
          />
        </Routes>
      </MemoryRouter>,
    );
    expect(screen.getByText("Original type label")).toBeInTheDocument();
    expect(screen.getByText("Retained named contact")).toBeInTheDocument();
    expect(screen.getByText("[REDACTED]")).toBeInTheDocument();
    expect(screen.queryByText("private-queue-fixture")).not.toBeInTheDocument();
  });
});
