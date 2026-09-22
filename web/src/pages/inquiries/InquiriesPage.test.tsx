import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { InquiryStatus } from "@/api";
import InquiriesPage from "./InquiriesPage";

const useInquiries = vi.fn();

vi.mock("@/hooks/useInquiries", () => ({
  useInquiries: (...args: unknown[]) => useInquiries(...args),
}));

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ user: { id: 7, login: "reviewer" } }),
}));

describe("InquiriesPage", () => {
  beforeEach(() => {
    useInquiries.mockReset();
    useInquiries.mockReturnValue({
      data: {
        items: [
          {
            id: 91,
            created_by_execution: 42,
            created_by_action_ref: "deploy.request_approval",
            assigned_to: 7,
            assigned_to_login: "reviewer",
            assigned_to_display_name: "Release Reviewer",
            workflow_root_execution: 40,
            workflow_action_ref: "deploy.production_release",
            workflow_task_name: "approval",
            prompt: "Approve production deployment?",
            status: InquiryStatus.PENDING,
            has_response: false,
            created: "2026-08-05T10:00:00Z",
            timeout_at: null,
          },
        ],
        pagination: {
          page: 1,
          page_size: 25,
          has_previous: false,
          has_next: true,
        },
      },
      isLoading: false,
      isFetching: false,
      error: null,
    });
  });

  it("defaults to pending and uses offset pagination", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={["/inquiries"]}>
        <InquiriesPage />
      </MemoryRouter>,
    );

    expect(useInquiries).toHaveBeenCalledWith({
      status: InquiryStatus.PENDING,
      createdByExecution: undefined,
      assignedTo: undefined,
      workflowActionRef: undefined,
      workflowPackRef: undefined,
      offset: 0,
      limit: 25,
    });
    expect(screen.getByRole("link", { name: "#91" })).toHaveAttribute(
      "href",
      "/inquiries/91",
    );
    expect(
      screen.getByRole("link", { name: "deploy.production_release" }),
    ).toHaveAttribute("href", "/executions/40");
    expect(screen.getByText("Release Reviewer (reviewer)")).toBeInTheDocument();

    await user.click(screen.getAllByRole("button", { name: "Next" })[0]);
    await waitFor(() =>
      expect(useInquiries).toHaveBeenLastCalledWith(
        expect.objectContaining({ offset: 25 }),
      ),
    );
  });

  it("filters by exact workflow action and pack references", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={["/inquiries"]}>
        <InquiriesPage />
      </MemoryRouter>,
    );

    await user.type(
      screen.getByRole("textbox", { name: "Workflow action" }),
      " deploy.production_release ",
    );
    await user.type(
      screen.getByRole("textbox", { name: "Workflow pack" }),
      " deploy ",
    );
    await user.click(screen.getByRole("button", { name: "Apply filters" }));

    await waitFor(() =>
      expect(useInquiries).toHaveBeenLastCalledWith(
        expect.objectContaining({
          workflowActionRef: "deploy.production_release",
          workflowPackRef: "deploy",
          offset: 0,
        }),
      ),
    );
  });

  it("filters inquiries assigned to the current user", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={["/inquiries"]}>
        <InquiriesPage />
      </MemoryRouter>,
    );

    await user.click(screen.getByRole("checkbox", { name: "Assigned to me" }));
    await user.click(screen.getByRole("button", { name: "Apply filters" }));

    await waitFor(() =>
      expect(useInquiries).toHaveBeenLastCalledWith(
        expect.objectContaining({ assignedTo: 7, offset: 0 }),
      ),
    );
  });

  it("keeps previous-page navigation on an empty later page", async () => {
    useInquiries.mockReturnValue({
      data: {
        items: [],
        pagination: {
          page: 2,
          page_size: 25,
          has_previous: true,
          has_next: false,
        },
      },
      isLoading: false,
      isFetching: false,
      error: null,
    });
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={["/inquiries?offset=25"]}>
        <InquiriesPage />
      </MemoryRouter>,
    );

    await user.click(screen.getAllByRole("button", { name: "Previous" })[0]);
    await waitFor(() =>
      expect(useInquiries).toHaveBeenLastCalledWith(
        expect.objectContaining({ offset: 0 }),
      ),
    );
  });
});
