import { useState, type ReactNode } from "react";
import { ArrowLeft, Clock, Loader2 } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import { ApiError, InquiryResponseOptionStyle } from "@/api";
import ParamSchemaForm, {
  type ParamSchema,
  validateParamSchema,
} from "@/components/common/ParamSchemaForm";
import { SchemaValueRows } from "@/components/common/CuratedDataPanel";
import { useAuth } from "@/contexts/AuthContext";
import { useInquiry, useRespondToInquiry } from "@/hooks/useInquiries";

function formatDate(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : "Not set";
}

function responseError(error: Error | null): string | null {
  if (!error) return null;
  if (error instanceof ApiError && error.status === 409) {
    return "This inquiry was already answered or is no longer pending. The latest state has been loaded.";
  }
  if (error instanceof ApiError && typeof error.body?.error === "string") {
    return error.body.error;
  }
  return error.message;
}

function identityLabel({
  id,
  login,
  displayName,
}: {
  id: number | null | undefined;
  login: string | null | undefined;
  displayName: string | null | undefined;
}): string | null {
  if (displayName && login && displayName !== login) {
    return `${displayName} (${login})`;
  }
  return displayName ?? login ?? (id ? `Identity #${id}` : null);
}

const optionClasses = {
  [InquiryResponseOptionStyle.DEFAULT]:
    "border-gray-300 bg-white text-gray-700 hover:bg-gray-50",
  [InquiryResponseOptionStyle.POSITIVE]:
    "border-blue-600 bg-blue-600 text-white hover:bg-blue-700",
  [InquiryResponseOptionStyle.DESTRUCTIVE]:
    "border-red-600 bg-red-600 text-white hover:bg-red-700",
};

export default function InquiryDetailPage() {
  const { user } = useAuth();
  const { id } = useParams<{ id: string }>();
  const inquiryId = Number(id);
  const validId = Number.isSafeInteger(inquiryId) && inquiryId > 0;
  const { data, isLoading, error } = useInquiry(inquiryId, validId);
  const respond = useRespondToInquiry(inquiryId);
  const [responseValues, setResponseValues] = useState<Record<string, unknown>>(
    {},
  );
  const [validationErrors, setValidationErrors] = useState<
    Record<string, string>
  >({});
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

  const canRespond =
    Boolean(user) &&
    inquiry.status === "pending" &&
    (inquiry.assigned_to == null || inquiry.assigned_to === user?.id);
  const responseSchema: ParamSchema = inquiry.response_schema ?? {};
  const hasResponseSchema = Object.keys(responseSchema).length > 0;
  const mutationError = responseError(respond.error);

  const submitCustomResponse = () => {
    const errors = validateParamSchema(responseSchema, responseValues);
    setValidationErrors(errors);
    if (Object.keys(errors).length === 0) {
      respond.mutate({ response: responseValues });
    }
  };

  return (
    <div className="p-6">
      <Link
        to="/inquiries"
        className="mb-4 inline-flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800"
      >
        <ArrowLeft className="h-4 w-4" />
        All inquiries
      </Link>

      <div className="mb-6 flex flex-wrap items-center gap-3">
        <h1 className="text-3xl font-bold text-gray-900">
          Inquiry #{inquiry.id}
        </h1>
        <span className="rounded-full bg-gray-100 px-3 py-1 text-sm font-medium capitalize text-gray-700">
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
            <Detail
              label="Creator execution"
              value={
                inquiry.created_by_execution > 0 ? (
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
                ) : null
              }
            />
            <Detail
              label="Workflow execution"
              value={
                inquiry.workflow_root_execution ? (
                  <div>
                    <Link
                      to={`/executions/${inquiry.workflow_root_execution}`}
                      className="text-blue-600 hover:text-blue-800"
                    >
                      {inquiry.workflow_action_ref ??
                        `Execution #${inquiry.workflow_root_execution}`}
                    </Link>
                    {inquiry.workflow_action_ref && (
                      <p className="mt-1 text-xs text-gray-500">
                        Execution #{inquiry.workflow_root_execution}
                      </p>
                    )}
                  </div>
                ) : null
              }
            />
            <Detail label="Workflow task" value={inquiry.workflow_task_name} />
            <Detail
              label="Assignee"
              value={
                identityLabel({
                  id: inquiry.assigned_to,
                  login: inquiry.assigned_to_login,
                  displayName: inquiry.assigned_to_display_name,
                }) ?? "Unassigned"
              }
            />
            <Detail
              label="Responder"
              value={identityLabel({
                id: inquiry.responded_by,
                login: inquiry.responded_by_login,
                displayName: inquiry.responded_by_display_name,
              })}
            />
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
            <div className="mt-3">
              <SchemaValueRows
                schema={inquiry.response_schema}
                values={inquiry.response}
                emptyMessage="No response fields were provided."
                maskSecrets
              />
            </div>
          </section>
        )}

        {canRespond &&
          (inquiry.response_options.length > 0 || hasResponseSchema) && (
            <section className="rounded-lg bg-white p-6 shadow">
              <h2 className="text-lg font-semibold text-gray-900">Respond</h2>

              {mutationError && (
                <p
                  role="alert"
                  className="mt-4 rounded-md border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700"
                >
                  {mutationError}
                </p>
              )}

              {inquiry.response_options.length > 0 && (
                <div className="mt-4 flex flex-wrap gap-3">
                  {inquiry.response_options.map((option) => (
                    <button
                      key={option.ref}
                      type="button"
                      disabled={respond.isPending}
                      onClick={() =>
                        respond.mutate({ response: option.response })
                      }
                      className={`rounded-md border px-4 py-2 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-60 ${optionClasses[option.style]}`}
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              )}

              {hasResponseSchema && (
                <div className="mt-5 border-t border-gray-200 pt-5">
                  <ParamSchemaForm
                    schema={responseSchema}
                    values={responseValues}
                    onChange={setResponseValues}
                    errors={validationErrors}
                    disabled={respond.isPending}
                  />
                  <button
                    type="button"
                    disabled={respond.isPending}
                    onClick={submitCustomResponse}
                    className="mt-4 inline-flex items-center gap-2 rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-60"
                  >
                    {respond.isPending && (
                      <Loader2 className="h-4 w-4 animate-spin" />
                    )}
                    Submit response
                  </button>
                </div>
              )}
            </section>
          )}

        {inquiry.status === "pending" && !canRespond && (
          <p className="flex items-center gap-2 text-sm text-amber-700">
            <Clock className="h-4 w-4" />
            This inquiry is assigned to another identity.
          </p>
        )}
      </div>
    </div>
  );
}

function Detail({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div>
      <dt className="text-sm font-medium text-gray-500">{label}</dt>
      <dd className="mt-1 text-sm text-gray-900">{value || "Not set"}</dd>
    </div>
  );
}
