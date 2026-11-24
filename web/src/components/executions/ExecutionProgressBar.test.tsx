import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useArtifact } from "@/hooks/useArtifacts";
import ExecutionProgressBar from "./ExecutionProgressBar";

vi.mock("@/hooks/useArtifactStream", () => ({
  useArtifactStream: vi.fn(),
}));

vi.mock("@/hooks/useArtifacts", () => ({
  useExecutionArtifacts: () => ({
    data: { items: [{ id: 7, type: "progress" }] },
  }),
  useArtifact: vi.fn(() => ({
    data: {
      data: {
        name: "Private artifact name",
        data: [{ percent: 30, message: "Latest authorized progress message" }],
      },
    },
  })),
}));

describe("ExecutionProgressBar", () => {
  it("renders progress from the execution-scoped artifact API", () => {
    render(<ExecutionProgressBar executionId={12} isRunning />);

    expect(useArtifact).toHaveBeenCalledWith(7, true);
    expect(screen.getByText("30%")).toBeInTheDocument();
    expect(screen.getByText("Private artifact name")).toBeInTheDocument();
    expect(
      screen.getByText("Latest authorized progress message"),
    ).toBeInTheDocument();
  });
});
