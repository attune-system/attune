import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Edit, Trash2 } from "lucide-react";
import { useDeletePolicy, usePolicy } from "@/hooks/usePolicies";
import { PolicyScopeType, type PolicyResponse } from "@/api/policies";

function formatScope(policy: PolicyResponse) {
  if (policy.scope.type === PolicyScopeType.ACTION) {
    return `Action: ${policy.scope.action_ref ?? "unknown"}`;
  }
  if (policy.scope.type === PolicyScopeType.PACK) {
    return `Pack: ${policy.scope.pack_ref ?? "unknown"}`;
  }
  return "Global";
}

export default function PolicyDetailPage() {
  const { ref } = useParams<{ ref: string }>();
  const navigate = useNavigate();
  const policyRef = ref ?? "";
  const { data, isLoading, error } = usePolicy(policyRef);
  const deletePolicy = useDeletePolicy();
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const policy = data?.data;

  const handleDelete = async () => {
    if (!policy) return;
    const confirmed = window.confirm(
      `Delete policy ${policy.ref}? This cannot be undone.`,
    );
    if (!confirmed) return;
    setDeleteError(null);
    try {
      await deletePolicy.mutateAsync(policy.ref);
      navigate("/policies");
    } catch (deleteFailure) {
      setDeleteError(
        deleteFailure instanceof Error
          ? deleteFailure.message
          : "Failed to delete policy",
      );
    }
  };

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex h-64 items-center justify-center">
          <div className="h-12 w-12 animate-spin rounded-full border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error || !policy) {
    return (
      <div className="mx-auto max-w-4xl p-6">
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-red-700">
          {error instanceof Error ? error.message : "Policy not found"}
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-6xl p-6">
      <div className="mb-6">
        <Link
          to="/policies"
          className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900"
        >
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back to Policies
        </Link>
        <div className="mt-4 flex items-start justify-between gap-4">
          <div>
            <h1 className="text-3xl font-bold text-gray-900">{policy.name}</h1>
            <p className="mt-1 font-mono text-sm text-gray-500">{policy.ref}</p>
            {policy.description && (
              <p className="mt-3 text-gray-600">{policy.description}</p>
            )}
          </div>
          <div className="flex gap-2">
            <Link
              to={`/policies/${encodeURIComponent(policy.ref)}/edit`}
              className="inline-flex items-center gap-2 rounded-lg border border-gray-300 px-4 py-2 text-gray-700 hover:bg-gray-50"
            >
              <Edit className="h-4 w-4" />
              Edit
            </Link>
            <button
              type="button"
              onClick={handleDelete}
              disabled={deletePolicy.isPending}
              className="inline-flex items-center gap-2 rounded-lg border border-red-200 px-4 py-2 text-red-700 hover:bg-red-50 disabled:opacity-60"
            >
              <Trash2 className="h-4 w-4" />
              Delete
            </button>
          </div>
        </div>
      </div>

      {deleteError && (
        <div className="mb-6 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {deleteError}
        </div>
      )}

      <div className="grid gap-6 md:grid-cols-4">
        <div className="rounded-lg bg-white p-5 shadow">
          <div className="text-sm font-medium text-gray-500">State</div>
          <div className="mt-2 text-lg font-semibold text-gray-900">
            {policy.enabled ? "Enabled" : "Disabled"}
          </div>
        </div>
        <div className="rounded-lg bg-white p-5 shadow">
          <div className="text-sm font-medium text-gray-500">Scope</div>
          <div className="mt-2 text-lg font-semibold text-gray-900">
            {formatScope(policy)}
          </div>
        </div>
        <div className="rounded-lg bg-white p-5 shadow">
          <div className="text-sm font-medium text-gray-500">Priority</div>
          <div className="mt-2 text-lg font-semibold text-gray-900">
            {policy.priority}
          </div>
        </div>
        <div className="rounded-lg bg-white p-5 shadow">
          <div className="text-sm font-medium text-gray-500">Features</div>
          <div className="mt-2 text-lg font-semibold text-gray-900">
            {[
              policy.concurrency ? "Concurrency" : null,
              policy.rate_limit ? "Rate limit" : null,
              policy.quotas.length > 0 ? "Quotas" : null,
            ]
              .filter(Boolean)
              .join(", ") || "None"}
          </div>
        </div>
      </div>

      <div className="mt-6 grid gap-6 lg:grid-cols-2">
        <section className="rounded-lg bg-white p-6 shadow">
          <h2 className="text-lg font-semibold text-gray-900">Concurrency</h2>
          {policy.concurrency ? (
            <dl className="mt-4 space-y-3 text-sm">
              <div className="flex justify-between gap-4">
                <dt className="text-gray-500">Limit</dt>
                <dd className="font-medium text-gray-900">
                  {policy.concurrency.limit}
                </dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-gray-500">Enforcement method</dt>
                <dd className="font-medium capitalize text-gray-900">
                  {policy.concurrency.method}
                </dd>
              </div>
              <div>
                <dt className="text-gray-500">Grouping paths</dt>
                <dd className="mt-1">
                  {(policy.concurrency.parameters ?? []).length > 0 ? (
                    <div className="flex flex-wrap gap-2">
                      {(policy.concurrency.parameters ?? []).map((path) => (
                        <span
                          key={path}
                          className="rounded-full bg-gray-100 px-2 py-1 font-mono text-xs text-gray-700"
                        >
                          {path}
                        </span>
                      ))}
                    </div>
                  ) : (
                    <span className="text-gray-500">Single shared limit</span>
                  )}
                </dd>
              </div>
            </dl>
          ) : (
            <p className="mt-3 text-sm text-gray-500">Not configured.</p>
          )}
        </section>

        <section className="rounded-lg bg-white p-6 shadow">
          <h2 className="text-lg font-semibold text-gray-900">Rate limit</h2>
          {policy.rate_limit ? (
            <dl className="mt-4 space-y-3 text-sm">
              <div className="flex justify-between gap-4">
                <dt className="text-gray-500">Max executions</dt>
                <dd className="font-medium text-gray-900">
                  {policy.rate_limit.max_executions}
                </dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-gray-500">Window</dt>
                <dd className="font-medium text-gray-900">
                  {policy.rate_limit.window_seconds} seconds
                </dd>
              </div>
            </dl>
          ) : (
            <p className="mt-3 text-sm text-gray-500">Not configured.</p>
          )}
        </section>

        <section className="rounded-lg bg-white p-6 shadow lg:col-span-2">
          <h2 className="text-lg font-semibold text-gray-900">Quotas</h2>
          {policy.quotas.length > 0 ? (
            <div className="mt-4 overflow-hidden rounded-lg border border-gray-200">
              <table className="min-w-full divide-y divide-gray-200 text-sm">
                <thead className="bg-gray-50">
                  <tr>
                    <th className="px-4 py-2 text-left font-medium text-gray-500">
                      Type
                    </th>
                    <th className="px-4 py-2 text-left font-medium text-gray-500">
                      Limit
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-200">
                  {policy.quotas.map((quota) => (
                    <tr key={quota.quota_type}>
                      <td className="px-4 py-2 font-mono">
                        {quota.quota_type}
                      </td>
                      <td className="px-4 py-2">{quota.limit}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="mt-3 text-sm text-gray-500">No quotas configured.</p>
          )}
        </section>
      </div>
    </div>
  );
}
