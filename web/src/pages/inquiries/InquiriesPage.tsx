import { FormEvent, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Inbox, Loader2 } from "lucide-react";
import { InquiryStatus } from "@/api";
import Pagination from "@/components/executions/Pagination";
import { useAuth } from "@/contexts/AuthContext";
import { useInquiries } from "@/hooks/useInquiries";

const PAGE_SIZE = 25;

function positiveInteger(value: string | null): number | undefined {
  if (!value) return undefined;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : undefined;
}

function inquiryStatus(value: string | null): InquiryStatus | undefined {
  return Object.values(InquiryStatus).find((status) => status === value);
}

function formatDate(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : "Not set";
}

function identityLabel({
  id,
  login,
  displayName,
}: {
  id: number | null | undefined;
  login: string | null | undefined;
  displayName: string | null | undefined;
}): string {
  if (displayName && login && displayName !== login) {
    return `${displayName} (${login})`;
  }
  return displayName ?? login ?? (id ? `Identity #${id}` : "Unassigned");
}

export default function InquiriesPage() {
  const { user } = useAuth();
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedStatus = searchParams.get("status");
  const status =
    selectedStatus === "all"
      ? undefined
      : (inquiryStatus(selectedStatus) ?? InquiryStatus.PENDING);
  const createdByExecution = positiveInteger(
    searchParams.get("created_by_execution"),
  );
  const assignedToMe = searchParams.get("assigned_to_me") === "true";
  const assignedTo = assignedToMe
    ? user?.id
    : positiveInteger(searchParams.get("assigned_to"));
  const workflowActionRef =
    searchParams.get("workflow_action_ref")?.trim() || undefined;
  const workflowPackRef =
    searchParams.get("workflow_pack_ref")?.trim() || undefined;
  const requestedOffset = Number(searchParams.get("offset"));
  const offset =
    Number.isSafeInteger(requestedOffset) && requestedOffset >= 0
      ? requestedOffset
      : 0;
  const page = Math.floor(offset / PAGE_SIZE) + 1;

  const [filters, setFilters] = useState({
    status: selectedStatus ?? InquiryStatus.PENDING,
    createdByExecution: searchParams.get("created_by_execution") ?? "",
    assignedTo: searchParams.get("assigned_to") ?? "",
    workflowActionRef: searchParams.get("workflow_action_ref") ?? "",
    workflowPackRef: searchParams.get("workflow_pack_ref") ?? "",
    assignedToMe,
  });

  const { data, isLoading, isFetching, error } = useInquiries({
    status,
    createdByExecution,
    assignedTo,
    workflowActionRef,
    workflowPackRef,
    offset,
    limit: PAGE_SIZE,
  });

  const inquiries = data?.items ?? [];
  const pagination = data?.pagination;

  const applyFilters = (event: FormEvent) => {
    event.preventDefault();
    const next = new URLSearchParams();
    next.set("status", filters.status);
    if (positiveInteger(filters.createdByExecution)) {
      next.set("created_by_execution", filters.createdByExecution);
    }
    if (filters.assignedToMe) {
      next.set("assigned_to_me", "true");
    } else if (positiveInteger(filters.assignedTo)) {
      next.set("assigned_to", filters.assignedTo);
    }
    if (filters.workflowActionRef.trim()) {
      next.set("workflow_action_ref", filters.workflowActionRef.trim());
    }
    if (filters.workflowPackRef.trim()) {
      next.set("workflow_pack_ref", filters.workflowPackRef.trim());
    }
    setSearchParams(next);
  };

  const setPage = (nextPage: number) => {
    const next = new URLSearchParams(searchParams);
    const nextOffset = (nextPage - 1) * PAGE_SIZE;
    if (nextOffset === 0) next.delete("offset");
    else next.set("offset", String(nextOffset));
    setSearchParams(next);
  };

  return (
    <div className="p-6">
      <div className="mb-6">
        <h1 className="text-3xl font-bold text-gray-900">Inquiries</h1>
        <p className="mt-1 text-sm text-gray-600">
          Review workflow questions and submit pending responses.
        </p>
      </div>

      <form
        onSubmit={applyFilters}
        className="mb-6 grid gap-4 rounded-lg bg-white p-5 shadow sm:grid-cols-2 xl:grid-cols-4"
      >
        <label className="text-sm font-medium text-gray-700">
          Status
          <select
            aria-label="Status"
            value={filters.status}
            onChange={(event) =>
              setFilters((current) => ({
                ...current,
                status: event.target.value,
              }))
            }
            className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2"
          >
            <option value={InquiryStatus.PENDING}>Pending</option>
            <option value={InquiryStatus.RESPONDED}>Responded</option>
            <option value={InquiryStatus.TIMEOUT}>Timed out</option>
            <option value={InquiryStatus.CANCELLED}>Cancelled</option>
            <option value="all">All statuses</option>
          </select>
        </label>
        <label className="text-sm font-medium text-gray-700">
          Workflow action
          <input
            aria-label="Workflow action"
            value={filters.workflowActionRef}
            onChange={(event) =>
              setFilters((current) => ({
                ...current,
                workflowActionRef: event.target.value,
              }))
            }
            className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2"
            placeholder="pack.workflow"
          />
        </label>
        <label className="text-sm font-medium text-gray-700">
          Workflow pack
          <input
            aria-label="Workflow pack"
            value={filters.workflowPackRef}
            onChange={(event) =>
              setFilters((current) => ({
                ...current,
                workflowPackRef: event.target.value,
              }))
            }
            className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2"
            placeholder="Any pack"
          />
        </label>
        <label className="text-sm font-medium text-gray-700">
          Creator execution
          <input
            aria-label="Creator execution"
            type="number"
            min="1"
            value={filters.createdByExecution}
            onChange={(event) =>
              setFilters((current) => ({
                ...current,
                createdByExecution: event.target.value,
              }))
            }
            className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2"
            placeholder="Any execution"
          />
        </label>
        <label className="text-sm font-medium text-gray-700">
          Assignee
          <input
            aria-label="Assignee"
            type="number"
            min="1"
            value={filters.assignedTo}
            disabled={filters.assignedToMe}
            onChange={(event) =>
              setFilters((current) => ({
                ...current,
                assignedTo: event.target.value,
              }))
            }
            className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2 disabled:bg-gray-100"
            placeholder="Any identity"
          />
        </label>
        <label className="flex items-center gap-2 self-end pb-2 text-sm font-medium text-gray-700">
          <input
            type="checkbox"
            checked={filters.assignedToMe}
            onChange={(event) =>
              setFilters((current) => ({
                ...current,
                assignedToMe: event.target.checked,
              }))
            }
            className="h-4 w-4 rounded border-gray-300 text-blue-600"
          />
          Assigned to me
        </label>
        <button
          type="submit"
          className="self-end rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700"
        >
          Apply filters
        </button>
      </form>

      {error && (
        <div className="mb-4 rounded-md border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          Failed to load inquiries: {error.message}
        </div>
      )}

      {isLoading ? (
        <div className="flex h-64 items-center justify-center gap-2 text-gray-500">
          <Loader2 className="h-5 w-5 animate-spin" />
          Loading inquiries...
        </div>
      ) : inquiries.length === 0 ? (
        <div className="overflow-hidden rounded-lg bg-white shadow">
          <div className="p-12 text-center">
            <Inbox className="mx-auto h-10 w-10 text-gray-400" />
            <p className="mt-3 text-gray-600">
              No inquiries match these filters.
            </p>
          </div>
          <Pagination
            page={page}
            setPage={setPage}
            pageSize={PAGE_SIZE}
            itemCount={0}
            total={pagination?.total_items ?? undefined}
            hasPrevious={pagination?.has_previous}
            hasNext={pagination?.has_next}
            itemLabel="inquiries"
          />
        </div>
      ) : (
        <div className="relative overflow-hidden rounded-lg bg-white shadow">
          {isFetching && (
            <div className="absolute inset-0 z-10 flex items-center justify-center bg-white/60">
              <Loader2 className="h-7 w-7 animate-spin text-blue-600" />
            </div>
          )}
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-200">
              <thead className="bg-gray-50">
                <tr>
                  {[
                    "Inquiry",
                    "Status",
                    "Creator execution",
                    "Workflow",
                    "Assignee",
                    "Created",
                    "Expires",
                  ].map((label) => (
                    <th
                      key={label}
                      className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500"
                    >
                      {label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-200">
                {inquiries.map((inquiry) => (
                  <tr key={inquiry.id} className="hover:bg-gray-50">
                    <td className="max-w-md px-6 py-4">
                      <Link
                        to={`/inquiries/${inquiry.id}`}
                        className="font-medium text-blue-600 hover:text-blue-800"
                      >
                        #{inquiry.id}
                      </Link>
                      <p className="mt-1 truncate text-sm text-gray-700">
                        {inquiry.prompt}
                      </p>
                    </td>
                    <td className="px-6 py-4 text-sm capitalize text-gray-700">
                      {inquiry.status}
                    </td>
                    <td className="px-6 py-4 text-sm text-gray-700">
                      {inquiry.created_by_execution > 0 ? (
                        <div>
                          <Link
                            to={`/executions/${inquiry.created_by_execution}`}
                            className="text-blue-600 hover:text-blue-800"
                          >
                            {inquiry.created_by_action_ref ??
                              `Execution #${inquiry.created_by_execution}`}
                          </Link>
                          {inquiry.created_by_action_ref && (
                            <p className="mt-1 text-xs text-gray-500">
                              Execution #{inquiry.created_by_execution}
                            </p>
                          )}
                        </div>
                      ) : (
                        "Not set"
                      )}
                    </td>
                    <td className="px-6 py-4 text-sm text-gray-700">
                      {inquiry.workflow_root_execution ? (
                        <div>
                          <Link
                            to={`/executions/${inquiry.workflow_root_execution}`}
                            className="text-blue-600 hover:text-blue-800"
                          >
                            {inquiry.workflow_action_ref ??
                              `Execution #${inquiry.workflow_root_execution}`}
                          </Link>
                          {inquiry.workflow_task_name && (
                            <p className="mt-1 text-xs text-gray-500">
                              Task {inquiry.workflow_task_name}
                            </p>
                          )}
                        </div>
                      ) : (
                        "Not set"
                      )}
                    </td>
                    <td className="px-6 py-4 text-sm text-gray-700">
                      {identityLabel({
                        id: inquiry.assigned_to,
                        login: inquiry.assigned_to_login,
                        displayName: inquiry.assigned_to_display_name,
                      })}
                    </td>
                    <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-700">
                      {formatDate(inquiry.created)}
                    </td>
                    <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-700">
                      {formatDate(inquiry.timeout_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Pagination
            page={page}
            setPage={setPage}
            pageSize={PAGE_SIZE}
            itemCount={inquiries.length}
            total={pagination?.total_items ?? undefined}
            hasPrevious={pagination?.has_previous}
            hasNext={pagination?.has_next}
            itemLabel="inquiries"
          />
        </div>
      )}
    </div>
  );
}
