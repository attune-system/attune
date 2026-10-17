import { useState } from "react";
import { PermissionScope } from "@/api";
import { toJsonValue } from "@/lib/json-value";
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
  parseGrants,
  permissionSetAuthoring,
} from "@/components/access-control/grants";
import PermissionScopeControl from "@/components/access-control/PermissionScopeControl";
import { GrantsEditor } from "@/components/access-control/GrantsEditor";
import { GrantsView } from "@/components/access-control/GrantsView";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";
import {
  draftToGrant,
  type GrantDraft,
  grantToDraft,
} from "@/pages/access-control/grantDraft";
import { safeErrorSummary } from "@/lib/safe-error-summary";

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
  const [authorityScope, setAuthorityScope] = useState<"pack" | "external">(
    "external",
  );
  const [editError, setEditError] = useState<string | null>(null);

  const permissionSets = permissionSetsRaw as
    PermissionSetWithRoles[] | undefined;
  const permissionSet = permissionSets?.find((ps) => ps.ref === ref);
  const authored = permissionSetAuthoring(permissionSet);
  const canManagePermissions = hasPermission(user, "permissions", "manage");

  const startEditingGrants = () => {
    if (!authored) return;
    setDraftGrants(authored.grants.map(grantToDraft));
    setAuthorityScope(authored.scope);
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
          grants: toJsonValue(grants),
          scope:
            authorityScope === "pack"
              ? PermissionScope.PACK
              : PermissionScope.EXTERNAL,
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
      console.error("Failed to add role:", safeErrorSummary(err));
    }
  };

  const handleDeleteRole = async (assignmentId: number, roleName: string) => {
    if (window.confirm(`Remove role "${roleName}" from this permission set?`)) {
      try {
        await deleteRoleAssignment.mutateAsync(assignmentId);
      } catch (err) {
        console.error(
          "Failed to delete role assignment:",
          safeErrorSummary(err),
        );
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
                disabled={!authored}
                title={
                  !authored
                    ? "Authored policy is unavailable; normalized IAM grants cannot be edited as authored scope."
                    : undefined
                }
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
            <div className="space-y-4">
              <PermissionScopeControl
                value={authorityScope}
                onChange={setAuthorityScope}
              />
              <GrantsEditor drafts={draftGrants} onChange={setDraftGrants} />
            </div>
          ) : (
            <GrantsView grants={parsedGrants} />
          )}
        </div>
      </div>
    </div>
  );
}
