import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import EnforcementDetailPage from "./EnforcementDetailPage";

vi.mock("@/hooks/useEvents", () => ({
  useEnforcement: () => ({
    isLoading: false,
    data: {
      data: {
        id: 1,
        status: "processed",
        condition: "all",
        conditions: {},
        rule_ref: "demo.rule",
        trigger_ref: "demo.trigger",
        event: 2,
        created: "2026-10-14T00:00:00Z",
        config: { count: 0, credential: "private-enforcement-fixture" },
        payload: { label: "Original event value" },
        input_contract: {
          annotated_schema: {
            type: "object",
            properties: {
              count: {
                type: "integer",
                description: "Original admitted count",
              },
              credential: { $ref: "#/$defs/credential" },
            },
            $defs: { credential: { type: "string", secret: true } },
          },
        },
        payload_contract: {
          annotated_schema: {
            type: "object",
            properties: {
              label: { type: "string", description: "Original event label" },
            },
          },
        },
      },
    },
  }),
}));

describe("enforcement stored contracts", () => {
  it("renders admitted input and event schemas without fetching current components", () => {
    render(
      <MemoryRouter initialEntries={["/enforcements/1"]}>
        <Routes>
          <Route path="/enforcements/:id" element={<EnforcementDetailPage />} />
        </Routes>
      </MemoryRouter>,
    );
    expect(screen.getByText("Original admitted count")).toBeInTheDocument();
    expect(screen.getByText("Original event label")).toBeInTheDocument();
    expect(screen.getByText("Original event value")).toBeInTheDocument();
    expect(screen.getByText("[REDACTED]")).toBeInTheDocument();
    expect(
      screen.queryByText("private-enforcement-fixture"),
    ).not.toBeInTheDocument();
  });
});
