import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Clock, Loader2 } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import { InquiriesService } from "@/api";

function formatDate(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : "Not set";
}

export default function InquiryDetailPage() {
  const { id } = useParams<{ id: string }>();
  const inquiryId = Number(id);
  const validId = Number.isSafeInteger(inquiryId) && inquiryId > 0;
  const { data, isLoading, error } = useQuery({
    queryKey: ["inquiries", inquiryId],
    queryFn: () => InquiriesService.getInquiry({ id: inquiryId }),
    enabled: validId,
  });
  const inquiry = data?.data;

  if (!validId) {
    return (
      <div className="p-6">
        <p className="rounded-lg border border-red-200 bg-red-50 p-4 text-red-700">
          Invalid inquiry ID.
        </p>
      </div>
    );
  }

  if (isLoading) {
    return (
      <div className="flex h-64 items-center justify-center gap-2 text-gray-500">
        <Loader2 className="h-5 w-5 animate-spin" />
        Loading inquiry...
      </div>
    );
  }

  if (error || !inquiry) {
    return (
      <div className="p-6">
        <div className="rounded-lg border border-red-200 bg-red-50 p-6">
          <h1 className="text-lg font-semibold text-red-900">
            Failed to load inquiry
          </h1>
          <p className="mt-2 text-sm text-red-700">
            {error instanceof Error ? error.message : "Inquiry not found"}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-6">
      <Link
        to={`/executions/${inquiry.execution}`}
        className="mb-4 inline-flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800"
      >
        <ArrowLeft className="h-4 w-4" />
        Execution #{inquiry.execution}
      </Link>

      <div className="mb-6 flex flex-wrap items-center gap-3">
        <h1 className="text-3xl font-bold text-gray-900">
          Inquiry #{inquiry.id}
        </h1>
        <span className="rounded-full bg-gray-100 px-3 py-1 text-sm font-medium text-gray-700">
          {inquiry.status}
        </span>
      </div>

      <div className="max-w-4xl space-y-6">
        <section className="rounded-lg bg-white p-6 shadow">
          <h2 className="text-sm font-semibold uppercase tracking-wide text-gray-500">
            Prompt
          </h2>
          <p className="mt-3 whitespace-pre-wrap text-gray-900">
            {inquiry.prompt}
          </p>
          {inquiry.purpose && (
            <p className="mt-3 text-sm text-gray-600">{inquiry.purpose}</p>
          )}
        </section>

        <section className="rounded-lg bg-white p-6 shadow">
          <h2 className="text-lg font-semibold text-gray-900">Details</h2>
          <dl className="mt-4 grid gap-4 sm:grid-cols-2">
            <Detail label="Workflow task" value={inquiry.workflow_task_name} />
            <Detail label="Created" value={formatDate(inquiry.created)} />
            <Detail label="Updated" value={formatDate(inquiry.updated)} />
            <Detail label="Expires" value={formatDate(inquiry.timeout_at)} />
            <Detail
              label="Responded"
              value={formatDate(inquiry.responded_at)}
            />
          </dl>
        </section>

        {inquiry.response != null && (
          <section className="rounded-lg bg-white p-6 shadow">
            <h2 className="text-lg font-semibold text-gray-900">Response</h2>
            <pre className="mt-3 overflow-x-auto rounded-md bg-gray-900 p-4 text-sm text-gray-100">
              {JSON.stringify(inquiry.response, null, 2)}
            </pre>
          </section>
        )}

        {inquiry.status === "pending" && (
          <p className="flex items-center gap-2 text-sm text-amber-700">
            <Clock className="h-4 w-4" />
            This page is read-only. The inquiry is still awaiting a response.
          </p>
        )}
      </div>
    </div>
  );
}

function Detail({
  label,
  value,
}: {
  label: string;
  value: string | null | undefined;
}) {
  return (
    <div>
      <dt className="text-sm font-medium text-gray-500">{label}</dt>
      <dd className="mt-1 text-sm text-gray-900">{value || "Not set"}</dd>
    </div>
  );
}
