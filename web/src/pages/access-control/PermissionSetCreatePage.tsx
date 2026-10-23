import { useState, type FormEvent } from "react";
import { PermissionScope } from "@/api";
import { Link, useNavigate } from "react-router-dom";
import { ArrowLeft, Plus, Shield } from "lucide-react";
import { GrantsEditor } from "@/components/access-control/GrantsEditor";
import PermissionScopeControl from "@/components/access-control/PermissionScopeControl";
import { useAuth } from "@/contexts/AuthContext";
import { useCreatePermissionSet } from "@/hooks/usePermissions";
import { toJsonValue } from "@/lib/json-value";
import { hasPermission } from "@/lib/permissions";
import { draftToGrant, type GrantDraft } from "./grantDraft";

const LIST_PATH = "/access-control?tab=permission-sets";

export default function PermissionSetCreatePage() {
  const { user } = useAuth();
  const navigate = useNavigate();
  const createPermissionSet = useCreatePermissionSet();
  const [ref, setRef] = useState("");
  const [label, setLabel] = useState("");
  const [description, setDescription] = useState("");
  const [drafts, setDrafts] = useState<GrantDraft[]>([]);
  const [authorityScope, setAuthorityScope] = useState<"pack" | "external">(
    "external",
  );
  const [error, setError] = useState<string | null>(null);
  const canManagePermissions = hasPermission(user, "permissions", "manage");

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault();
    if (!canManagePermissions || createPermissionSet.isPending) return;
    setError(null);
    try {
      const response = await createPermissionSet.mutateAsync({
        ref: ref.trim(),
        label: label.trim() || null,
        description: description.trim() || null,
        grants: toJsonValue(drafts.map(draftToGrant)),
        scope:
          authorityScope === "pack"
            ? PermissionScope.PACK
            : PermissionScope.EXTERNAL,
      });
      navigate(
        `/access-control/permission-sets/${encodeURIComponent(response.data.ref)}`,
      );
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to create permission set.",
      );
    }
  };

  return (
    <div className="p-6">
      <Link
        to={LIST_PATH}
        className="mb-6 inline-flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800"
      >
        <ArrowLeft className="h-4 w-4" />
        Back to Access Control
      </Link>
      <div className="mb-6 rounded-lg bg-white p-6 shadow">
        <div className="flex items-center gap-4">
          <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-lg bg-indigo-100">
            <Shield className="h-6 w-6 text-indigo-600" />
          </div>
          <div>
            <h1 className="text-2xl font-bold text-gray-900">
              Create Permission Set
            </h1>
            <p className="mt-1 text-sm text-gray-500">
              Define grants, then assign roles or identities after creation.
            </p>
          </div>
        </div>
      </div>
      {!canManagePermissions ? (
        <div
          role="alert"
          className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700"
        >
          You need permissions:manage to create permission sets.
        </div>
      ) : (
        <form onSubmit={handleSubmit} className="space-y-6">
          {error && (
            <div
              role="alert"
              className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700"
            >
              {error}
            </div>
          )}
          <fieldset
            disabled={createPermissionSet.isPending}
            className="space-y-6"
          >
            <div className="space-y-4 rounded-lg bg-white p-6 shadow">
              <h2 className="text-lg font-semibold text-gray-900">Details</h2>
              <div>
                <label
                  htmlFor="permission-set-ref"
                  className="mb-1 block text-sm font-medium text-gray-700"
                >
                  Reference <span className="text-red-500">*</span>
                </label>
                <input
                  id="permission-set-ref"
                  value={ref}
                  onChange={(event) => setRef(event.target.value)}
                  required
                  maxLength={255}
                  pattern={String.raw`[a-z][a-z0-9_\-]*\.[a-z][a-z0-9_\-]*`}
                  title="Use namespace.name with lowercase letters, digits, hyphens, or underscores. Each part must start with a letter."
                  placeholder="custom.operator"
                  aria-describedby="permission-set-ref-help"
                  className="w-full rounded-md border border-gray-300 px-3 py-2 font-mono text-sm"
                />
                <p
                  id="permission-set-ref-help"
                  className="mt-1 text-xs text-gray-500"
                >
                  Use namespace.name. This creates a standalone permission set,
                  not a pack-owned definition.
                </p>
              </div>
              <div>
                <label
                  htmlFor="permission-set-label"
                  className="mb-1 block text-sm font-medium text-gray-700"
                >
                  Label
                </label>
                <input
                  id="permission-set-label"
                  value={label}
                  onChange={(event) => setLabel(event.target.value)}
                  maxLength={255}
                  placeholder="Operator"
                  className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                />
              </div>
              <div>
                <label
                  htmlFor="permission-set-description"
                  className="mb-1 block text-sm font-medium text-gray-700"
                >
                  Description
                </label>
                <textarea
                  id="permission-set-description"
                  value={description}
                  onChange={(event) => setDescription(event.target.value)}
                  rows={3}
                  className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                />
              </div>
            </div>
            <div className="rounded-lg bg-white shadow">
              <div className="flex items-center gap-2 border-b border-gray-200 px-6 py-4">
                <Shield className="h-5 w-5 text-gray-500" />
                <h2 className="text-lg font-semibold text-gray-900">Grants</h2>
                <span className="text-sm text-gray-500">({drafts.length})</span>
              </div>
              <PermissionScopeControl
                value={authorityScope}
                onChange={setAuthorityScope}
              />
              <GrantsEditor drafts={drafts} onChange={setDrafts} />
            </div>
          </fieldset>
          <div className="flex justify-end gap-3">
            <button
              type="button"
              onClick={() => navigate(LIST_PATH)}
              disabled={createPermissionSet.isPending}
              className="rounded-md bg-gray-100 px-4 py-2 text-sm text-gray-700 hover:bg-gray-200 disabled:opacity-50"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={!ref.trim() || createPermissionSet.isPending}
              className="inline-flex items-center gap-2 rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50"
            >
              <Plus className="h-4 w-4" />
              {createPermissionSet.isPending
                ? "Creating..."
                : "Create Permission Set"}
            </button>
          </div>
        </form>
      )}
    </div>
  );
}
