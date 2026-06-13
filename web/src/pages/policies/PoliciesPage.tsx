import { useMemo, useState } from "react";
import { Pencil, Plus, ShieldCheck, Trash2, X } from "lucide-react";

import { useAuth } from "@/contexts/AuthContext";
import {
  useCreatePolicy,
  useDeletePolicy,
  usePolicies,
  useUpdatePolicy,
} from "@/hooks/usePolicies";
import { hasPermission } from "@/lib/permissions";
import type {
  CreatePolicyRequest,
  PolicyMethod,
  PolicyResponse,
  PolicyScope,
} from "@/api/policies";

const INPUT_CLASS =
  "w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500/30 disabled:bg-gray-100";

interface PolicyDraft {
  ref: string;
  scope: PolicyScope;
  target: string;
  method: PolicyMethod;
  threshold: string;
  parameters: string;
  name: string;
  description: string;
  tags: string;
}

const emptyDraft: PolicyDraft = {
  ref: "",
  scope: "action",
  target: "",
  method: "enqueue",
  threshold: "1",
  parameters: "",
  name: "",
  description: "",
  tags: "",
};

function csv(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function targetLabel(policy: PolicyResponse): string {
  return policy.action_ref || policy.pack_ref || "Global";
}

function draftFromPolicy(policy: PolicyResponse): PolicyDraft {
  return {
    ref: policy.ref,
    scope: policy.scope,
    target: policy.action_ref || policy.pack_ref || "",
    method: policy.method,
    threshold: String(policy.threshold),
    parameters: policy.parameters.join(", "),
    name: policy.name,
    description: policy.description ?? "",
    tags: policy.tags.join(", "),
  };
}

function requestFromDraft(draft: PolicyDraft): CreatePolicyRequest {
  return {
    ref: draft.ref.trim(),
    pack_ref: draft.scope === "pack" ? draft.target.trim() : undefined,
    action_ref: draft.scope === "action" ? draft.target.trim() : undefined,
    parameters: csv(draft.parameters),
    method: draft.method,
    threshold: Number(draft.threshold),
    name: draft.name.trim(),
    description: draft.description.trim() || null,
    tags: csv(draft.tags),
  };
}

export default function PoliciesPage() {
  const { user } = useAuth();
  const canCreate = hasPermission(user, "policies", "create");
  const canUpdate = hasPermission(user, "policies", "update");
  const canDelete = hasPermission(user, "policies", "delete");
  const [scopeFilter, setScopeFilter] = useState<PolicyScope | "">("");
  const [packFilter, setPackFilter] = useState("");
  const [actionFilter, setActionFilter] = useState("");
  const [editing, setEditing] = useState<PolicyResponse | null>(null);
  const [draft, setDraft] = useState<PolicyDraft>(emptyDraft);
  const [formError, setFormError] = useState<string | null>(null);

  const policiesQuery = usePolicies({
    scope: scopeFilter || undefined,
    packRef: packFilter.trim(),
    actionRef: actionFilter.trim(),
  });
  const createPolicy = useCreatePolicy();
  const updatePolicy = useUpdatePolicy();
  const deletePolicy = useDeletePolicy();

  const policies = policiesQuery.data?.items ?? [];
  const sortedPolicies = useMemo(
    () => [...policies].sort((a, b) => a.ref.localeCompare(b.ref)),
    [policies],
  );
  const saving = createPolicy.isPending || updatePolicy.isPending;

  const startCreate = () => {
    setEditing(null);
    setDraft(emptyDraft);
    setFormError(null);
  };

  const startEdit = (policy: PolicyResponse) => {
    setEditing(policy);
    setDraft(draftFromPolicy(policy));
    setFormError(null);
  };

  const save = () => {
    const threshold = Number(draft.threshold);
    if (!draft.ref.trim() || !draft.name.trim()) {
      setFormError("Reference and name are required.");
      return;
    }
    if ((draft.scope === "pack" || draft.scope === "action") && !draft.target.trim()) {
      setFormError("A pack or action target is required for this scope.");
      return;
    }
    if (!Number.isInteger(threshold) || threshold <= 0) {
      setFormError("Threshold must be a positive integer.");
      return;
    }

    const request = requestFromDraft(draft);
    setFormError(null);
    if (editing) {
      updatePolicy.mutate({
        ref: editing.ref,
        data: {
          parameters: request.parameters,
          method: request.method,
          threshold: request.threshold,
          name: request.name,
          description: request.description,
          tags: request.tags,
        },
      });
    } else {
      createPolicy.mutate(request);
    }
  };

  const remove = (policy: PolicyResponse) => {
    if (window.confirm(`Delete policy '${policy.ref}'?`)) {
      deletePolicy.mutate(policy.ref);
    }
  };

  return (
    <div className="flex h-full min-h-0">
      <div className="w-[30rem] overflow-y-auto border-r border-gray-200 bg-gray-50">
        <div className="sticky top-0 z-10 border-b border-gray-200 bg-white p-4">
          <div className="flex items-center justify-between gap-3">
            <div>
              <h1 className="flex items-center gap-2 text-2xl font-bold text-gray-900">
                <ShieldCheck className="h-6 w-6 text-blue-600" />
                Policies
              </h1>
              <p className="mt-1 text-sm text-gray-600">
                {sortedPolicies.length} execution admission policies
              </p>
            </div>
            {canCreate && (
              <button
                type="button"
                onClick={startCreate}
                className="inline-flex items-center gap-1.5 rounded-md bg-blue-600 px-3 py-2 text-sm font-medium text-white hover:bg-blue-700"
              >
                <Plus className="h-4 w-4" />
                New
              </button>
            )}
          </div>
          <div className="mt-4 grid grid-cols-1 gap-2">
            <select
              className={INPUT_CLASS}
              value={scopeFilter}
              onChange={(event) => setScopeFilter(event.target.value as PolicyScope | "")}
            >
              <option value="">All scopes</option>
              <option value="global">Global</option>
              <option value="pack">Pack</option>
              <option value="action">Action</option>
            </select>
            <input
              className={INPUT_CLASS}
              placeholder="Filter pack ref"
              value={packFilter}
              onChange={(event) => setPackFilter(event.target.value)}
            />
            <input
              className={INPUT_CLASS}
              placeholder="Filter action ref"
              value={actionFilter}
              onChange={(event) => setActionFilter(event.target.value)}
            />
          </div>
        </div>

        {policiesQuery.isLoading ? (
          <div className="flex h-48 items-center justify-center">
            <div className="h-8 w-8 animate-spin rounded-full border-b-2 border-blue-600" />
          </div>
        ) : policiesQuery.error ? (
          <div className="m-4 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700">
            Failed to load policies.
          </div>
        ) : sortedPolicies.length === 0 ? (
          <div className="p-6 text-center text-sm text-gray-500">No policies found.</div>
        ) : (
          <div className="divide-y divide-gray-200">
            {sortedPolicies.map((policy) => (
              <button
                key={policy.ref}
                type="button"
                onClick={() => startEdit(policy)}
                className={`block w-full px-4 py-3 text-left hover:bg-white ${
                  editing?.ref === policy.ref ? "bg-blue-50" : ""
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <div className="font-medium text-gray-900">{policy.ref}</div>
                    <div className="text-sm text-gray-600">{policy.name}</div>
                  </div>
                  <span className="rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-700">
                    {policy.scope}
                  </span>
                </div>
                <div className="mt-2 text-xs text-gray-500">
                  {targetLabel(policy)} · {policy.method} · limit {policy.threshold}
                </div>
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="min-w-0 flex-1 overflow-y-auto p-6">
        <div className="mx-auto max-w-3xl space-y-6">
          <div>
            <h2 className="text-xl font-semibold text-gray-900">
              {editing ? "Edit policy" : "Create policy"}
            </h2>
            <p className="mt-1 text-sm text-gray-600">
              Policies define concurrency admission limits separately from action metadata.
            </p>
          </div>

          <div className="rounded-lg border border-gray-200 bg-white p-5 shadow-sm">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <label className="space-y-1 md:col-span-2">
                <span className="text-sm font-medium text-gray-700">Reference</span>
                <input
                  className={INPUT_CLASS}
                  value={draft.ref}
                  disabled={!!editing}
                  placeholder="core.echo_concurrency"
                  onChange={(event) => setDraft({ ...draft, ref: event.target.value })}
                />
              </label>
              <label className="space-y-1 md:col-span-2">
                <span className="text-sm font-medium text-gray-700">Name</span>
                <input
                  className={INPUT_CLASS}
                  value={draft.name}
                  onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                />
              </label>
              <label className="space-y-1">
                <span className="text-sm font-medium text-gray-700">Scope</span>
                <select
                  className={INPUT_CLASS}
                  value={draft.scope}
                  disabled={!!editing}
                  onChange={(event) =>
                    setDraft({ ...draft, scope: event.target.value as PolicyScope, target: "" })
                  }
                >
                  <option value="global">Global</option>
                  <option value="pack">Pack</option>
                  <option value="action">Action</option>
                </select>
              </label>
              <label className="space-y-1">
                <span className="text-sm font-medium text-gray-700">Target</span>
                <input
                  className={INPUT_CLASS}
                  value={draft.target}
                  disabled={draft.scope === "global" || !!editing}
                  placeholder={draft.scope === "pack" ? "core" : "core.echo"}
                  onChange={(event) => setDraft({ ...draft, target: event.target.value })}
                />
              </label>
              <label className="space-y-1">
                <span className="text-sm font-medium text-gray-700">Method</span>
                <select
                  className={INPUT_CLASS}
                  value={draft.method}
                  onChange={(event) =>
                    setDraft({ ...draft, method: event.target.value as PolicyMethod })
                  }
                >
                  <option value="enqueue">Enqueue</option>
                  <option value="cancel">Cancel</option>
                </select>
              </label>
              <label className="space-y-1">
                <span className="text-sm font-medium text-gray-700">Threshold</span>
                <input
                  className={INPUT_CLASS}
                  type="number"
                  min={1}
                  value={draft.threshold}
                  onChange={(event) => setDraft({ ...draft, threshold: event.target.value })}
                />
              </label>
              <label className="space-y-1 md:col-span-2">
                <span className="text-sm font-medium text-gray-700">Parameter groups</span>
                <input
                  className={INPUT_CLASS}
                  value={draft.parameters}
                  placeholder="customer_id, environment"
                  onChange={(event) => setDraft({ ...draft, parameters: event.target.value })}
                />
              </label>
              <label className="space-y-1 md:col-span-2">
                <span className="text-sm font-medium text-gray-700">Description</span>
                <textarea
                  className={INPUT_CLASS}
                  rows={3}
                  value={draft.description}
                  onChange={(event) => setDraft({ ...draft, description: event.target.value })}
                />
              </label>
              <label className="space-y-1 md:col-span-2">
                <span className="text-sm font-medium text-gray-700">Tags</span>
                <input
                  className={INPUT_CLASS}
                  value={draft.tags}
                  placeholder="operator-managed, production"
                  onChange={(event) => setDraft({ ...draft, tags: event.target.value })}
                />
              </label>
            </div>

            {formError && (
              <div className="mt-4 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700">
                {formError}
              </div>
            )}

            <div className="mt-5 flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={save}
                disabled={saving || (editing ? !canUpdate : !canCreate)}
                className="inline-flex items-center gap-2 rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:bg-gray-400"
              >
                <Pencil className="h-4 w-4" />
                {editing ? "Save policy" : "Create policy"}
              </button>
              <button
                type="button"
                onClick={startCreate}
                className="inline-flex items-center gap-2 rounded-md border border-gray-300 px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50"
              >
                <X className="h-4 w-4" />
                Reset
              </button>
              {editing && canDelete && (
                <button
                  type="button"
                  onClick={() => remove(editing)}
                  disabled={deletePolicy.isPending}
                  className="ml-auto inline-flex items-center gap-2 rounded-md border border-red-300 px-4 py-2 text-sm font-medium text-red-700 hover:bg-red-50 disabled:opacity-50"
                >
                  <Trash2 className="h-4 w-4" />
                  Delete
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
