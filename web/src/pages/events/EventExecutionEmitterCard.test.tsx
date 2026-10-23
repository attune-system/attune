import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import type { EventExecutionEmitterResponse } from "@/api";
import EventExecutionEmitterCard from "./EventExecutionEmitterCard";

describe("EventExecutionEmitterCard", () => {
  it("does not infer an emitter when the server omits its projection", () => {
    const { container } = render(
      <EventExecutionEmitterCard emitter={undefined} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("shows historical admission separately from requester and sensor source", () => {
    const emitter = {
      execution_id: 42,
      admission_id: 51,
      sponsor_identity_id: 61,
      pack_id: 71,
      pack_ref: "example",
      action_ref: "example.emit",
      policy_revision_id: 81,
      release_id: 91,
      release_digest: "a".repeat(64),
      attempt: "00000000-0000-4000-8000-000000000001",
    } satisfies EventExecutionEmitterResponse;
    render(
      <MemoryRouter>
        <EventExecutionEmitterCard emitter={emitter} />
      </MemoryRouter>,
    );
    expect(screen.getByRole("link", { name: "#42" }).getAttribute("href")).toBe(
      "/executions/42",
    );
    expect(screen.getByText("example.emit")).toBeTruthy();
    expect(screen.getByText("Runtime sponsor ID")).toBeTruthy();
    expect(screen.getByText(emitter.release_digest)).toBeTruthy();
    expect(
      screen.getByText(/execution may no longer be retained/),
    ).toBeTruthy();
  });
});
