import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Gauge, Plus, Search } from "lucide-react";
import Pagination from "@/components/executions/Pagination";
import { usePolicies } from "@/hooks/usePolicies";
import {
  PolicyMethod,
  PolicyScopeType,
  type PolicySummary,
} from "@/api/policies";

function FeatureBadges({ policy }: { policy: PolicySummary }) {
  return (
    <div className="flex flex-wrap gap-1">
      {policy.concurrency && (
        <span className="rounded-full bg-blue-100 px-2 py-0.5 text-xs font-medium text-blue-800">
          Concurrency {policy.concurrency.limit} / {policy.concurrency.method}
        </span>
      )}
      {policy.rate_limit && (
        <span className="rounded-full bg-purple-100 px-2 py-0.5 text-xs font-medium text-purple-800">
          {policy.rate_limit.max_executions}/{policy.rate_limit.window_seconds}s
        </span>
      )}
      {policy.quotas.length > 0 && (
        <span className="rounded-full bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-800">
          {policy.quotas.length} quota{policy.quotas.length === 1 ? "" : "s"}
        </span>
      )}
      {!policy.concurrency && !policy.rate_limit && policy.quotas.length === 0 && (
        <span className="rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-700">
          No features
        </span>
      )}
    </div>
  );
}

function scopeLabel(policy: PolicySummary): string {
  if (policy.scope.type === PolicyScopeType.ACTION) {
    return `Action: ${policy.scope.action_ref ?? "unknown"}`;
  }
  if (policy.scope.type === PolicyScopeType.PACK) {
    return `Pack: ${policy.scope.pack_ref ?? "unknown"}`;
  }
  return "Global";
}

export default function PoliciesPage() {
  const [page, setPage] = useState(1);
  const [search, setSearch] = useState("");
  const [scope, setScope] = useState<"all" | PolicyScopeType>("all");
  const [enabledFilter, setEnabledFilter] = useState<
    "all" | "enabled" | "disabled"
  >("all");
  const [method, setMethod] = useState<"all" | PolicyMethod>("all");
  const pageSize = 20;
  const queryParams = useMemo(
    () => ({
      page,
      pageSize,
      scope: scope === "all" ? undefined : scope,
      enabled:
        enabledFilter === "all" ? undefined : enabledFilter === "enabled",
    }),
    [enabledFilter, page, scope],
  );
  const { data, isLoading, error, isFetching } = usePolicies(queryParams);
  const normalizedSearch = search.trim().toLowerCase();
  const policies = (data?.items ?? []).filter((policy) => {
    const matchesSearch =
      normalizedSearch.length === 0 ||
      policy.ref.toLowerCase().includes(normalizedSearch) ||
      policy.name.toLowerCase().includes(normalizedSearch) ||
      (policy.description ?? "").toLowerCase().includes(normalizedSearch) ||
      policy.tags.some((tag) => tag.toLowerCase().includes(normalizedSearch));
    const matchesMethod =
      method === "all" || policy.concurrency?.method === method;
    return matchesSearch && matchesMethod;
  });
  const pagination = data?.pagination;
  const total = pagination?.total_items ?? policies.length;
  const hasActiveFilters =
    search.trim().length > 0 ||
    scope !== "all" ||
    enabledFilter !== "all" ||
    method !== "all";

  return (
    <div className="p-6">
      <div className="mb-6 flex items-center justify-between gap-4">
        <div>
          <h1 className="text-3xl font-bold text-gray-900">Policies</h1>
          <p className="mt-2 text-gray-600">
            Configure execution concurrency, rate limits, and quotas across
            global, pack, and action scopes.
          </p>
        </div>
        <Link
          to="/policies/new"
          className="inline-flex items-center gap-2 rounded-lg bg-blue-600 px-4 py-2 text-white transition-colors hover:bg-blue-700"
        >
          <Plus className="h-4 w-4" />
          Create Policy
        </Link>
      </div>

      <div className="mb-6 rounded-lg bg-white p-4 shadow">
        <div className="grid gap-4 md:grid-cols-4">
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              <span className="inline-flex items-center gap-2">
                <Search className="h-4 w-4" />
                Search policies
              </span>
            </label>
            <input
              value={search}
              onChange={(event) => {
                setSearch(event.target.value);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Search by ref, name, tag"
            />
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Scope
            </label>
            <select
              value={scope}
              onChange={(event) => {
                setScope(event.target.value as typeof scope);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="all">All scopes</option>
              <option value={PolicyScopeType.GLOBAL}>Global</option>
              <option value={PolicyScopeType.PACK}>Pack</option>
              <option value={PolicyScopeType.ACTION}>Action</option>
            </select>
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              State
            </label>
            <select
              value={enabledFilter}
              onChange={(event) => {
                setEnabledFilter(event.target.value as typeof enabledFilter);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="all">All states</option>
              <option value="enabled">Enabled</option>
              <option value="disabled">Disabled</option>
            </select>
          </div>
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Concurrency behavior
            </label>
            <select
              value={method}
              onChange={(event) => {
                setMethod(event.target.value as typeof method);
                setPage(1);
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            >
              <option value="all">All behaviors</option>
              <option value={PolicyMethod.ENQUEUE}>Enqueue</option>
              <option value={PolicyMethod.CANCEL}>Cancel</option>
            </select>
          </div>
        </div>
        <div className="mt-4 flex items-center justify-between">
          <div className="text-sm text-gray-600">
            {policies.length > 0
              ? `Showing ${policies.length} of ${total} policies`
              : "No policies found"}
            {isFetching && !isLoading ? " - refreshing..." : ""}
          </div>
          {hasActiveFilters && (
            <button
              type="button"
              onClick={() => {
                setSearch("");
                setScope("all");
                setEnabledFilter("all");
                setMethod("all");
                setPage(1);
              }}
              className="text-sm text-gray-600 hover:text-gray-900"
            >
              Clear filters
            </button>
          )}
        </div>
      </div>

      {error && (
        <div className="mb-6 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error instanceof Error ? error.message : "Failed to load policies"}
        </div>
      )}

      <div className="overflow-hidden rounded-lg bg-white shadow">
        {isLoading ? (
          <div className="p-12 text-center text-gray-600">Loading policies...</div>
        ) : policies.length === 0 ? (
          <div className="p-12 text-center">
            <Gauge className="mx-auto h-10 w-10 text-gray-400" />
            <h3 className="mt-3 text-lg font-medium text-gray-900">
              No policies found
            </h3>
            <p className="mt-1 text-gray-500">
              Create a policy to control execution throughput and limits.
            </p>
          </div>
        ) : (
          <table className="min-w-full divide-y divide-gray-200">
            <thead className="bg-gray-50">
              <tr>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  Policy
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  Scope
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  Features
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  Priority
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  State
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-200 bg-white">
              {policies.map((policy) => (
                <tr key={policy.ref} className="hover:bg-gray-50">
                  <td className="px-6 py-4">
                    <Link
                      to={`/policies/${encodeURIComponent(policy.ref)}`}
                      className="font-medium text-blue-600 hover:text-blue-800"
                    >
                      {policy.name}
                    </Link>
                    <div className="mt-1 font-mono text-xs text-gray-500">
                      {policy.ref}
                    </div>
                  </td>
                  <td className="px-6 py-4 text-sm text-gray-700">
                    {scopeLabel(policy)}
                  </td>
                  <td className="px-6 py-4">
                    <FeatureBadges policy={policy} />
                  </td>
                  <td className="px-6 py-4 text-sm text-gray-700">
                    {policy.priority}
                  </td>
                  <td className="px-6 py-4">
                    <span
                      className={`rounded-full px-2 py-1 text-xs font-medium ${
                        policy.enabled
                          ? "bg-green-100 text-green-800"
                          : "bg-gray-100 text-gray-700"
                      }`}
                    >
                      {policy.enabled ? "Enabled" : "Disabled"}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      {pagination && (
        <div className="mt-6">
          <Pagination
            page={page}
            setPage={setPage}
            pageSize={pageSize}
            itemCount={policies.length}
            total={pagination.total_items ?? undefined}
            hasPrevious={pagination.has_previous}
            hasNext={pagination.has_next}
            itemLabel="policies"
          />
        </div>
      )}
    </div>
  );
}
