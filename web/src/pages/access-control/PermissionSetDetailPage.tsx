import { useState } from "react";
import { useParams, Link } from "react-router-dom";
import {
  ArrowLeft,
  Edit3,
  Package,
  Plus,
  Save,
  Shield,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import {
  usePermissionSets,
  useCreatePermissionSetRoleAssignment,
  useDeletePermissionSetRoleAssignment,
  useUpdatePermissionSet,
} from "@/hooks/usePermissions";
import {
  ACTION_STYLE,
  parseGrants,
  RESOURCE_META,
} from "@/components/access-control/grants";
import { GrantsView } from "@/components/access-control/GrantsView";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";
import {
  COMPONENT_SCOPED_RESOURCES,
  componentScopeFieldLabel,
  componentScopeOptionLabel,
  componentScopePlaceholder,
  draftToGrant,
  type GrantDraft,
  grantToDraft,
  newGrantDraft,
  normalizeDraft,
  OWNER_REF_RESOURCES,
  OWNER_SCOPED_RESOURCES,
  OWNER_TYPE_RESOURCES,
  PACK_SCOPED_RESOURCES,
  RESOURCE_ACTIONS,
  type ScopeType,
} from "@/pages/access-control/grantDraft";

// ── Domain interfaces ──────────────────────────────────────────────────────────

interface PermissionSetRoleAssignment {
  id: number;
  permission_set_id: number;
  permission_set_ref: string | null;
  role: string;
  created: string;
}

interface PermissionSetWithRoles {
  id: number;
  ref: string;
  pack_ref?: string | null;
  label?: string | null;
  description?: string | null;
  grants: unknown;
  roles?: PermissionSetRoleAssignment[];
}

const RESOURCE_OPTIONS = Object.keys(RESOURCE_ACTIONS).map((value) => ({
  value,
  label: RESOURCE_META[value]?.label ?? value,
}));

// ── Constraint chips ───────────────────────────────────────────────────────────

function GrantsEditor({
  drafts,
  onChange,
}: {
  drafts: GrantDraft[];
  onChange: (drafts: GrantDraft[]) => void;
}) {
  const updateDraft = (id: string, patch: Partial<GrantDraft>) => {
    onChange(
      drafts.map((draft) =>
        draft.id === id ? normalizeDraft({ ...draft, ...patch }) : draft,
      ),
    );
  };

  const toggleAction = (draft: GrantDraft, action: string) => {
    const nextActions = draft.actions.includes(action)
      ? draft.actions.filter((entry) => entry !== action)
      : [...draft.actions, action];
    updateDraft(draft.id, { actions: nextActions });
  };

  const toggleVisibility = (draft: GrantDraft, visibility: string) => {
    const nextVisibility = draft.visibility.includes(visibility)
      ? draft.visibility.filter((entry) => entry !== visibility)
      : [...draft.visibility, visibility];
    updateDraft(draft.id, { visibility: nextVisibility });
  };

  return (
    <div className="space-y-4 p-4">
      {drafts.length === 0 ? (
        <div className="rounded-lg border border-dashed border-gray-300 p-8 text-center text-sm text-gray-500">
          No grants configured. Add a grant to begin.
        </div>
      ) : (
        drafts.map((draft, index) => {
          const meta = RESOURCE_META[draft.resource];
          const Icon = meta?.icon ?? Shield;
          const validActions = RESOURCE_ACTIONS[draft.resource] ?? [];
          const canPackScope = PACK_SCOPED_RESOURCES.has(draft.resource);
          const canComponentScope = COMPONENT_SCOPED_RESOURCES.has(
            draft.resource,
          );
          const canScope = canPackScope || canComponentScope;
          const showOwner = OWNER_SCOPED_RESOURCES.has(draft.resource);
          const showOwnerType = OWNER_TYPE_RESOURCES.has(draft.resource);
          const showOwnerRefs = OWNER_REF_RESOURCES.has(draft.resource);
          const showVisibility = draft.resource === "artifacts";
          const showExecutionScope = draft.resource === "executions";
          const showEncrypted = draft.resource === "keys";
          return (
            <div
              key={draft.id}
              className="rounded-lg border border-gray-200 bg-gray-50 p-4"
            >
              <div className="mb-4 flex items-start justify-between gap-4">
                <div className="flex items-center gap-2">
                  <Icon
                    className={`h-5 w-5 ${meta?.color ?? "text-gray-500"}`}
                  />
                  <div>
                    <div className="text-sm font-semibold text-gray-900">
                      Grant {index + 1}
                    </div>
                    <div className="text-xs text-gray-500">
                      Select a resource, permission specs, and optional scope.
                    </div>
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() =>
                    onChange(drafts.filter((entry) => entry.id !== draft.id))
                  }
                  className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs text-red-600 hover:bg-red-50"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  Remove
                </button>
              </div>

              <div className="grid gap-4 lg:grid-cols-[14rem,1fr]">
                <div>
                  <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                    Resource
                  </label>
                  <select
                    value={draft.resource}
                    onChange={(event) => {
                      updateDraft(draft.id, {
                        resource: event.target.value,
                        actions: RESOURCE_ACTIONS[event.target.value]?.includes(
                          "read",
                        )
                          ? ["read"]
                          : [RESOURCE_ACTIONS[event.target.value]?.[0]].filter(
                              Boolean,
                            ),
                        scopeType: "unconstrained",
                        scopeRefs: "",
                      });
                    }}
                    className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                  >
                    {RESOURCE_OPTIONS.map((resource) => (
                      <option key={resource.value} value={resource.value}>
                        {resource.label}
                      </option>
                    ))}
                  </select>
                </div>

                <div>
                  <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                    Permission specs
                  </label>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {validActions.map((action) => (
                      <label
                        key={action}
                        className={`inline-flex cursor-pointer items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium ${
                          draft.actions.includes(action)
                            ? (ACTION_STYLE[action] ??
                              "bg-gray-100 text-gray-700")
                            : "bg-white text-gray-500 ring-1 ring-gray-200"
                        }`}
                      >
                        <input
                          type="checkbox"
                          checked={draft.actions.includes(action)}
                          onChange={() => toggleAction(draft, action)}
                          className="h-3 w-3"
                        />
                        {action}
                      </label>
                    ))}
                  </div>
                </div>
              </div>

              <div className="mt-5 rounded-md border border-gray-200 bg-white p-4">
                <div className="mb-3 text-xs font-semibold uppercase tracking-wide text-gray-500">
                  Resource scope
                </div>
                {canScope ? (
                  <div className="grid gap-4 md:grid-cols-[14rem,1fr]">
                    <div>
                      <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                        Scope type
                      </label>
                      <select
                        value={draft.scopeType}
                        onChange={(event) =>
                          updateDraft(draft.id, {
                            scopeType: event.target.value as ScopeType,
                            scopeRefs: "",
                          })
                        }
                        className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                      >
                        <option value="unconstrained">Unconstrained</option>
                        {canPackScope && (
                          <option value="pack">Pack scoped</option>
                        )}
                        {canComponentScope && (
                          <option value="component">
                            {componentScopeOptionLabel(draft.resource)}
                          </option>
                        )}
                      </select>
                    </div>
                    <ScopeInput
                      label={
                        draft.scopeType === "pack"
                          ? "Pack refs"
                          : componentScopeFieldLabel(draft.resource)
                      }
                      value={
                        draft.scopeType === "unconstrained"
                          ? ""
                          : draft.scopeRefs
                      }
                      placeholder={
                        draft.scopeType === "pack"
                          ? "core, slack"
                          : componentScopePlaceholder(draft.resource)
                      }
                      disabled={draft.scopeType === "unconstrained"}
                      onChange={(value) =>
                        updateDraft(draft.id, { scopeRefs: value })
                      }
                    />
                  </div>
                ) : (
                  <p className="text-sm text-gray-500">
                    This resource currently supports only unconstrained grants.
                  </p>
                )}

                {(showOwner ||
                  showOwnerType ||
                  showOwnerRefs ||
                  showVisibility ||
                  showExecutionScope ||
                  showEncrypted) && (
                  <div className="mt-5 grid gap-4 md:grid-cols-2">
                    {showOwner && (
                      <div>
                        <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                          Owner
                        </label>
                        <select
                          value={draft.owner}
                          onChange={(event) =>
                            updateDraft(draft.id, { owner: event.target.value })
                          }
                          className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                        >
                          <option value="">Any / not constrained</option>
                          <option value="self">Own resources</option>
                          <option value="any">Any owner</option>
                          <option value="none">No owner</option>
                        </select>
                      </div>
                    )}

                    {showOwnerType && (
                      <ScopeInput
                        label="Owner types"
                        value={draft.ownerTypes}
                        placeholder="identity, pack, action, sensor"
                        onChange={(value) =>
                          updateDraft(draft.id, { ownerTypes: value })
                        }
                      />
                    )}

                    {showOwnerRefs && (
                      <ScopeInput
                        label="Owner refs"
                        value={draft.ownerRefs}
                        placeholder="salesforce, my-sensor"
                        onChange={(value) =>
                          updateDraft(draft.id, { ownerRefs: value })
                        }
                      />
                    )}

                    {showExecutionScope && (
                      <div>
                        <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                          Execution scope
                        </label>
                        <select
                          value={draft.executionScope}
                          onChange={(event) =>
                            updateDraft(draft.id, {
                              executionScope: event.target.value,
                            })
                          }
                          className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                        >
                          <option value="">Any / not constrained</option>
                          <option value="self">Own executions</option>
                          <option value="descendants">Own + descendants</option>
                          <option value="any">All executions</option>
                        </select>
                      </div>
                    )}

                    {showEncrypted && (
                      <div>
                        <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                          Key encryption scope
                        </label>
                        <select
                          value={draft.encrypted}
                          onChange={(event) =>
                            updateDraft(draft.id, {
                              encrypted: event.target.value,
                            })
                          }
                          className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                        >
                          <option value="">Any / not constrained</option>
                          <option value="true">Encrypted only</option>
                          <option value="false">Unencrypted only</option>
                        </select>
                      </div>
                    )}
                  </div>
                )}

                {showVisibility && (
                  <div className="mt-4">
                    <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                      Artifact visibility
                    </label>
                    <div className="mt-2 flex gap-2">
                      {["public", "private"].map((visibility) => (
                        <label
                          key={visibility}
                          className="inline-flex items-center gap-2 rounded-md border border-gray-200 bg-white px-3 py-1.5 text-sm text-gray-700"
                        >
                          <input
                            type="checkbox"
                            checked={draft.visibility.includes(visibility)}
                            onChange={() => toggleVisibility(draft, visibility)}
                          />
                          {visibility}
                        </label>
                      ))}
                    </div>
                  </div>
                )}

                <div className="mt-4">
                  <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                    Identity attribute constraints JSON
                  </label>
                  <p className="mt-1 text-xs text-gray-500">
                    Optional ABAC filter. Every key/value here must exactly
                    match the requesting identity's stored attributes for the
                    grant to apply.
                  </p>
                  <textarea
                    value={draft.attributes}
                    onChange={(event) =>
                      updateDraft(draft.id, { attributes: event.target.value })
                    }
                    placeholder='{"environment": "prod"}'
                    rows={3}
                    className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 font-mono text-xs"
                  />
                </div>
              </div>
            </div>
          );
        })
      )}

      <button
        type="button"
        onClick={() => onChange([...drafts, newGrantDraft()])}
        className="inline-flex items-center gap-2 rounded-md border border-dashed border-blue-300 px-3 py-2 text-sm font-medium text-blue-700 hover:bg-blue-50"
      >
        <Plus className="h-4 w-4" />
        Add grant
      </button>
    </div>
  );
}

function ScopeInput({
  label,
  value,
  placeholder,
  disabled = false,
  onChange,
}: {
  label: string;
  value: string;
  placeholder: string;
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <div>
      <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
        {label}
      </label>
      <input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm disabled:cursor-not-allowed disabled:bg-gray-100 disabled:text-gray-400"
      />
      <p className="mt-1 text-xs text-gray-400">Comma-separated</p>
    </div>
  );
}

// ── Page ───────────────────────────────────────────────────────────────────────

export default function PermissionSetDetailPage() {
  const { ref } = useParams<{ ref: string }>();
  const { user } = useAuth();

  const { data: permissionSetsRaw, isLoading, error } = usePermissionSets();
  const updatePermissionSet = useUpdatePermissionSet();
  const createRoleAssignment = useCreatePermissionSetRoleAssignment();
  const deleteRoleAssignment = useDeletePermissionSetRoleAssignment();

  const [newRole, setNewRole] = useState("");
  const [showAddRole, setShowAddRole] = useState(false);
  const [isEditingGrants, setIsEditingGrants] = useState(false);
  const [draftGrants, setDraftGrants] = useState<GrantDraft[]>([]);
  const [editError, setEditError] = useState<string | null>(null);

  const permissionSets = permissionSetsRaw as
    PermissionSetWithRoles[] | undefined;
  const permissionSet = permissionSets?.find((ps) => ps.ref === ref);
  const canManagePermissions = hasPermission(user, "permissions", "manage");

  const startEditingGrants = () => {
    setDraftGrants(parseGrants(permissionSet?.grants).map(grantToDraft));
    setEditError(null);
    setIsEditingGrants(true);
  };

  const cancelEditingGrants = () => {
    setDraftGrants([]);
    setEditError(null);
    setIsEditingGrants(false);
  };

  const handleSaveGrants = async () => {
    if (!permissionSet) {
      return;
    }

    setEditError(null);
    try {
      const grants = draftGrants.map(draftToGrant);
      await updatePermissionSet.mutateAsync({
        id: permissionSet.id,
        data: {
          label: permissionSet.label ?? null,
          description: permissionSet.description ?? null,
          grants,
        },
      });
      setIsEditingGrants(false);
      setDraftGrants([]);
    } catch (err) {
      setEditError(
        err instanceof Error ? err.message : "Failed to update permission set.",
      );
    }
  };

  const handleAddRole = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newRole.trim()) return;
    try {
      await createRoleAssignment.mutateAsync({
        permissionSetId: permissionSet!.id,
        role: newRole.trim(),
      });
      setNewRole("");
      setShowAddRole(false);
    } catch (err) {
      console.error("Failed to add role:", err);
    }
  };

  const handleDeleteRole = async (assignmentId: number, roleName: string) => {
    if (window.confirm(`Remove role "${roleName}" from this permission set?`)) {
      try {
        await deleteRoleAssignment.mutateAsync(assignmentId);
      } catch (err) {
        console.error("Failed to delete role assignment:", err);
      }
    }
  };

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="text-center">
            <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600 mx-auto"></div>
            <p className="mt-3 text-sm text-gray-500">
              Loading permission set…
            </p>
          </div>
        </div>
      </div>
    );
  }

  if (error || !permissionSet) {
    return (
      <div className="p-6">
        <Link
          to="/access-control?tab=permission-sets"
          className="inline-flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800 mb-6"
        >
          <ArrowLeft className="w-4 h-4" />
          Back to Access Control
        </Link>
        <div className="bg-white rounded-lg shadow p-12 text-center">
          <Shield className="mx-auto h-12 w-12 text-gray-400" />
          <p className="mt-4 text-red-600">
            {error
              ? "Failed to load permission set"
              : "Permission set not found"}
          </p>
        </div>
      </div>
    );
  }

  const roles = permissionSet.roles || [];
  const parsedGrants = parseGrants(permissionSet.grants);

  return (
    <div className="p-6">
      {/* Back link */}
      <Link
        to="/access-control?tab=permission-sets"
        className="inline-flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800 mb-6"
      >
        <ArrowLeft className="w-4 h-4" />
        Back to Access Control
      </Link>

      {/* Header */}
      <div className="bg-white rounded-lg shadow p-6 mb-6">
        <div className="flex items-start justify-between">
          <div className="flex items-center gap-4">
            <div className="flex-shrink-0 w-12 h-12 bg-indigo-100 rounded-lg flex items-center justify-center">
              <Shield className="w-6 h-6 text-indigo-600" />
            </div>
            <div>
              <h1 className="text-2xl font-bold text-gray-900 font-mono">
                {permissionSet.ref}
              </h1>
              {permissionSet.label && (
                <p className="text-lg text-gray-700 mt-0.5">
                  {permissionSet.label}
                </p>
              )}
              {permissionSet.description && (
                <p className="text-sm text-gray-500 mt-1">
                  {permissionSet.description}
                </p>
              )}
            </div>
          </div>
          {permissionSet.pack_ref && (
            <span className="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-sm font-medium bg-green-100 text-green-800">
              <Package className="w-3.5 h-3.5" />
              {permissionSet.pack_ref}
            </span>
          )}
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Roles Section */}
        <div className="bg-white rounded-lg shadow">
          <div className="px-6 py-4 border-b border-gray-200 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Tag className="w-5 h-5 text-gray-500" />
              <h2 className="text-lg font-semibold text-gray-900">
                Role Assignments
              </h2>
              <span className="text-sm text-gray-500">({roles.length})</span>
            </div>
            <button
              onClick={() => setShowAddRole(!showAddRole)}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-blue-600 hover:text-blue-800 hover:bg-blue-50 rounded-md transition-colors"
            >
              <Plus className="w-4 h-4" />
              Add Role
            </button>
          </div>

          {showAddRole && (
            <form
              onSubmit={handleAddRole}
              className="px-6 py-3 bg-blue-50 border-b border-blue-100 flex items-center gap-3"
            >
              <input
                type="text"
                value={newRole}
                onChange={(e) => setNewRole(e.target.value)}
                placeholder="Enter role name…"
                className="flex-1 px-3 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                autoFocus
              />
              <button
                type="submit"
                disabled={!newRole.trim() || createRoleAssignment.isPending}
                className="px-3 py-1.5 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 rounded-md disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
              >
                {createRoleAssignment.isPending ? "Adding…" : "Add"}
              </button>
              <button
                type="button"
                onClick={() => {
                  setShowAddRole(false);
                  setNewRole("");
                }}
                className="px-3 py-1.5 text-sm text-gray-600 hover:text-gray-900"
              >
                Cancel
              </button>
            </form>
          )}

          {createRoleAssignment.isError && (
            <div className="px-6 py-2 bg-red-50 border-b border-red-100">
              <p className="text-sm text-red-600">
                Failed to add role.{" "}
                {createRoleAssignment.error instanceof Error
                  ? createRoleAssignment.error.message
                  : "Please try again."}
              </p>
            </div>
          )}

          <div className="divide-y divide-gray-100">
            {roles.length === 0 ? (
              <div className="px-6 py-8 text-center">
                <Tag className="mx-auto h-8 w-8 text-gray-300" />
                <p className="mt-2 text-sm text-gray-500">
                  No roles assigned to this permission set
                </p>
                <p className="text-xs text-gray-400 mt-1">
                  Identities with a matching role will inherit these grants
                </p>
              </div>
            ) : (
              roles.map((assignment) => (
                <div
                  key={assignment.id}
                  className="px-6 py-3 flex items-center justify-between hover:bg-gray-50"
                >
                  <div className="flex items-center gap-3">
                    <span className="inline-flex items-center px-2.5 py-1 rounded-full text-sm font-medium bg-purple-100 text-purple-800">
                      {assignment.role}
                    </span>
                    <span className="text-xs text-gray-400">
                      Added {new Date(assignment.created).toLocaleDateString()}
                    </span>
                  </div>
                  <button
                    onClick={() =>
                      handleDeleteRole(assignment.id, assignment.role)
                    }
                    className="text-red-400 hover:text-red-600 p-1 rounded transition-colors"
                    title="Remove role assignment"
                  >
                    <Trash2 className="w-4 h-4" />
                  </button>
                </div>
              ))
            )}
          </div>
        </div>

        {/* Grants Section */}
        <div className="bg-white rounded-lg shadow">
          <div className="px-6 py-4 border-b border-gray-200 flex items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <Shield className="w-5 h-5 text-gray-500" />
              <h2 className="text-lg font-semibold text-gray-900">Grants</h2>
              <span className="text-sm text-gray-500">
                ({parsedGrants.length})
              </span>
            </div>
            {canManagePermissions && !isEditingGrants && (
              <button
                type="button"
                onClick={startEditingGrants}
                className="inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium text-blue-600 hover:bg-blue-50 hover:text-blue-800"
              >
                <Edit3 className="h-4 w-4" />
                Edit grants
              </button>
            )}
            {isEditingGrants && (
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={cancelEditingGrants}
                  className="inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm text-gray-600 hover:bg-gray-100"
                >
                  <X className="h-4 w-4" />
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={handleSaveGrants}
                  disabled={updatePermissionSet.isPending}
                  className="inline-flex items-center gap-1.5 rounded-md bg-blue-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <Save className="h-4 w-4" />
                  {updatePermissionSet.isPending ? "Saving..." : "Save"}
                </button>
              </div>
            )}
          </div>
          {editError && (
            <div className="border-b border-red-100 bg-red-50 px-6 py-3 text-sm text-red-700">
              {editError}
            </div>
          )}
          {isEditingGrants ? (
            <GrantsEditor drafts={draftGrants} onChange={setDraftGrants} />
          ) : (
            <GrantsView grants={parsedGrants} />
          )}
        </div>
      </div>
    </div>
  );
}
