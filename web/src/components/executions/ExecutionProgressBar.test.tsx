import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useArtifact } from "@/hooks/useArtifacts";
import ExecutionProgressBar from "./ExecutionProgressBar";

vi.mock("@/hooks/useArtifactStream", () => ({
  useArtifactStream: vi.fn(),
  useArtifactProgress: () => ({ artifactId: 7, percent: 60, entries: 2 }),
}));

vi.mock("@/hooks/useArtifacts", () => ({
  useExecutionArtifacts: () => ({ data: { items: [] } }),
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
  it("combines numeric notification progress with authorized artifact details", () => {
    render(<ExecutionProgressBar executionId={12} isRunning />);

    expect(useArtifact).toHaveBeenCalledWith(7, true);
    expect(screen.getByText("60%")).toBeInTheDocument();
    expect(screen.getByText("Private artifact name")).toBeInTheDocument();
    expect(
      screen.getByText("Latest authorized progress message"),
    ).toBeInTheDocument();
  });
});
