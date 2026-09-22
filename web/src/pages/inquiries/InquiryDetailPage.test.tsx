import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, InquiryResponseOptionStyle } from "@/api";
import InquiryDetailPage from "./InquiryDetailPage";

const getInquiry = vi.fn();
const respondToInquiry = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    InquiriesService: {
      ...actual.InquiriesService,
      getInquiry: (...args: unknown[]) => getInquiry(...args),
      respondToInquiry: (...args: unknown[]) => respondToInquiry(...args),
    },
  };
});

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ user: { id: 7, login: "reviewer" } }),
}));

function Wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
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

function inquiry(overrides: Record<string, unknown> = {}) {
  return {
    id: 91,
    created_by_execution: 42,
    created_by_action_ref: "deploy.request_approval",
    created_by_pack_ref: "deploy",
    prompt: "Approve production deployment?",
    purpose: "Release gate",
    response: null,
    response_schema: null,
    response_options: [],
    status: "pending",
    assigned_to: null,
    assigned_to_login: null,
    assigned_to_display_name: null,
    responded_by: null,
    responded_by_login: null,
    responded_by_display_name: null,
    workflow_execution: 43,
    workflow_root_execution: 40,
    workflow_action_ref: "deploy.production_release",
    workflow_pack_ref: "deploy",
    workflow_task_name: "deploy",
    created: "2026-08-05T10:00:00Z",
    updated: "2026-08-05T10:01:00Z",
    ...overrides,
  };
}

describe("InquiryDetailPage", () => {
  beforeEach(() => {
    getInquiry.mockReset();
    respondToInquiry.mockReset();
  });

  it("submits a fixed response and refetches the inquiry", async () => {
    getInquiry.mockResolvedValue({
      data: inquiry({
        response_options: [
          {
            ref: "approve",
            label: "Approve",
            response: { approved: true },
            style: InquiryResponseOptionStyle.POSITIVE,
          },
        ],
      }),
    });
    respondToInquiry.mockResolvedValue({
      data: inquiry({ status: "responded", response: { approved: true } }),
    });
    const user = userEvent.setup();

    render(<InquiryDetailPage />, { wrapper: Wrapper });
    await user.click(await screen.findByRole("button", { name: "Approve" }));

    expect(respondToInquiry).toHaveBeenCalledWith({
      id: 91,
      requestBody: { response: { approved: true } },
    });
    await waitFor(() => expect(getInquiry).toHaveBeenCalledTimes(2));
  });

  it("links the workflow root execution and shows identity labels", async () => {
    getInquiry.mockResolvedValue({
      data: inquiry({
        assigned_to: 7,
        assigned_to_login: "reviewer",
        assigned_to_display_name: "Release Reviewer",
        responded_by: 8,
        responded_by_login: "operator",
        responded_by_display_name: "Operations",
      }),
    });

    render(<InquiryDetailPage />, { wrapper: Wrapper });

    expect(
      await screen.findByRole("link", { name: "deploy.production_release" }),
    ).toHaveAttribute("href", "/executions/40");
    expect(screen.queryByText("Execution #43")).not.toBeInTheDocument();
    expect(screen.getByText("Release Reviewer (reviewer)")).toBeInTheDocument();
    expect(screen.getByText("Operations (operator)")).toBeInTheDocument();
  });

  it("renders response values with schema metadata and secret masking", async () => {
    getInquiry.mockResolvedValue({
      data: inquiry({
        status: "responded",
        response_schema: {
          approved: {
            type: "boolean",
            required: true,
            description: "Approval decision",
          },
          token: { type: "string", secret: true },
          reason: { type: "string" },
        },
        response: {
          approved: true,
          token: "redacted-by-api",
          reviewer_note: "Checked rollout plan",
        },
      }),
    });

    render(<InquiryDetailPage />, { wrapper: Wrapper });

    expect(await screen.findByText("Approval decision")).toBeInTheDocument();
    expect(screen.getByText("Not provided")).toBeInTheDocument();
    expect(screen.getByText("not in schema")).toBeInTheDocument();
    expect(screen.getByText("••••••••")).toBeInTheDocument();
    expect(screen.queryByText("redacted-by-api")).not.toBeInTheDocument();
  });

  it("shows a conflict inline and refetches the inquiry", async () => {
    getInquiry.mockResolvedValue({
      data: inquiry({
        response_options: [
          {
            ref: "approve",
            label: "Approve",
            response: { approved: true },
            style: InquiryResponseOptionStyle.POSITIVE,
          },
        ],
      }),
    });
    respondToInquiry.mockRejectedValue(
      new ApiError(
        { method: "POST", url: "/api/v1/inquiries/91/respond" },
        {
          url: "/api/v1/inquiries/91/respond",
          ok: false,
          status: 409,
          statusText: "Conflict",
          body: null,
        },
        "Conflict",
      ),
    );
    const user = userEvent.setup();

    render(<InquiryDetailPage />, { wrapper: Wrapper });
    await user.click(await screen.findByRole("button", { name: "Approve" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The latest state has been loaded.",
    );
    await waitFor(() => expect(getInquiry).toHaveBeenCalledTimes(2));
  });

  it("shows the API error detail for a rejected response", async () => {
    getInquiry.mockResolvedValue({
      data: inquiry({
        response_options: [
          {
            ref: "approve",
            label: "Approve",
            response: { approved: true },
            style: InquiryResponseOptionStyle.POSITIVE,
          },
        ],
      }),
    });
    respondToInquiry.mockRejectedValue(
      new ApiError(
        { method: "POST", url: "/api/v1/inquiries/91/respond" },
        {
          url: "/api/v1/inquiries/91/respond",
          ok: false,
          status: 403,
          statusText: "Forbidden",
          body: { error: "Only the assigned identity can respond" },
        },
        "Forbidden",
      ),
    );
    const user = userEvent.setup();

    render(<InquiryDetailPage />, { wrapper: Wrapper });
    await user.click(await screen.findByRole("button", { name: "Approve" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Only the assigned identity can respond",
    );
  });

  it("validates and submits a custom flat response schema", async () => {
    getInquiry.mockResolvedValue({
      data: inquiry({
        response_schema: {
          reason: {
            type: "string",
            required: true,
            description: "Reason for the decision",
          },
        },
      }),
    });
    respondToInquiry.mockResolvedValue({ data: inquiry() });
    const user = userEvent.setup();

    render(<InquiryDetailPage />, { wrapper: Wrapper });
    await screen.findByRole("heading", { name: "Respond" });
    await user.click(screen.getByRole("button", { name: "Submit response" }));

    expect(screen.getByText("This field is required")).toBeInTheDocument();
    expect(respondToInquiry).not.toHaveBeenCalled();

    await user.type(screen.getByRole("textbox"), "Approved after review");
    await user.click(screen.getByRole("button", { name: "Submit response" }));

    expect(respondToInquiry).toHaveBeenCalledWith({
      id: 91,
      requestBody: { response: { reason: "Approved after review" } },
    });
  });

  it("hides controls for an inquiry assigned to another identity", async () => {
    getInquiry.mockResolvedValue({
      data: inquiry({
        assigned_to: 8,
        created_by_execution: 0,
        workflow_execution: null,
        workflow_root_execution: null,
        response_options: [
          {
            ref: "approve",
            label: "Approve",
            response: { approved: true },
            style: InquiryResponseOptionStyle.POSITIVE,
          },
        ],
      }),
    });

    render(<InquiryDetailPage />, { wrapper: Wrapper });

    expect(
      await screen.findByText("This inquiry is assigned to another identity."),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Approve" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("link", { name: /Execution #0/ }),
    ).not.toBeInTheDocument();
  });
});
