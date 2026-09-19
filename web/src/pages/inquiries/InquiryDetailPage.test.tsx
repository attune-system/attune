import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import InquiryDetailPage from "./InquiryDetailPage";

const getInquiry = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    InquiriesService: {
      ...actual.InquiriesService,
      getInquiry: (...args: unknown[]) => getInquiry(...args),
    },
  };
});

function Wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={["/inquiries/91"]}>
        <Routes>
          <Route path="/inquiries/:id" element={children} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe("InquiryDetailPage", () => {
  it("loads and renders inquiry details without response controls", async () => {
    getInquiry.mockResolvedValue({
      data: {
        id: 91,
        execution: 42,
        prompt: "Approve production deployment?",
        purpose: "Release gate",
        response: null,
        response_schema: null,
        status: "pending",
        created: "2026-08-05T10:00:00Z",
        updated: "2026-08-05T10:01:00Z",
        workflow_task_name: "deploy",
      },
    });

    render(<InquiryDetailPage />, { wrapper: Wrapper });

    expect(
      await screen.findByRole("heading", { name: "Inquiry #91" }),
    ).toBeInTheDocument();
    expect(getInquiry).toHaveBeenCalledWith({ id: 91 });
    expect(
      screen.getByText("Approve production deployment?"),
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Execution #42/ })).toHaveAttribute(
      "href",
      "/executions/42",
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
