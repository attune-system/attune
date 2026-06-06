import { useMemo, useState } from "react";
import { useParams, Link } from "react-router-dom";
import {
  Shield,
  ArrowLeft,
  Trash2,
  Plus,
  Tag,
  Snowflake,
  Sun,
  ShieldCheck,
  User,
  FileJson,
  Search,
  KeyRound,
  Copy,
  ChevronDown,
  ChevronRight,
} from "lucide-react";
import type { PermissionSetSummary } from "@/api";
import {
  useIdentity,
  usePermissionSets,
  useCreateIdentityRoleAssignment,
  useDeleteIdentityRoleAssignment,
  useCreatePermissionAssignment,
  useDeletePermissionAssignment,
  useFreezeIdentity,
  useUnfreezeIdentity,
  useIntegrationTokens,
  useCreateIntegrationToken,
  useRevokeIntegrationToken,
  useDeleteIntegrationToken,
  type IntegrationToken,
} from "@/hooks/usePermissions";
import { GrantsView } from "@/components/access-control/GrantsView";
import {
  type ParsedGrant,
  parseGrants,
} from "@/components/access-control/grants";

interface RoleAssignment {
  id: number;
  identity_id: number;
  role: string;
  source: string;
  managed: boolean;
  created: string;
  updated: string;
}

interface DirectPermission {
  id: number;
  identity_id: number;
  permission_set_id: number;
  permission_set_ref: string;
  created: string;
}

interface IdentityDetail {
  id: number;
  login: string;
  display_name: string | null;
  frozen: boolean;
  attributes: Record<string, unknown>;
  roles: RoleAssignment[];
  direct_permissions: DirectPermission[];
}

interface PermissionSetInfo {
  ref: string;
  label?: string | null;
  pack_ref?: string | null;
  grants: ParsedGrant[];
}

type PermissionSetDescriptor = Pick<
  PermissionSetInfo,
  "ref" | "label" | "pack_ref"
>;

interface EffectivePermissionSet extends PermissionSetInfo {
  direct: boolean;
  viaRoles: string[];
}

interface EffectiveGrantRow {
  grant: ParsedGrant;
  permissionSets: EffectivePermissionSet[];
}

const ARRAY_CONSTRAINT_KEYS = new Set([
  "actions",
  "pack_refs",
  "owner_types",
  "visibility",
  "refs",
]);

function sortGrantValue(value: unknown, key?: string): unknown {
  if (Array.isArray(value)) {
    const normalized = value.map((item) => sortGrantValue(item));
    if (key && ARRAY_CONSTRAINT_KEYS.has(key)) {
      return [...normalized].sort((left, right) =>
        JSON.stringify(left).localeCompare(JSON.stringify(right)),
      );
    }
    return normalized;
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([entryKey, entryValue]) => [
          entryKey,
          sortGrantValue(entryValue, entryKey),
        ]),
    );
  }

  return value;
}

function getGrantKey(grant: ParsedGrant): string {
  return JSON.stringify(sortGrantValue(grant));
}

function getPermissionSetPackName(
  permissionSet: PermissionSetDescriptor,
): string {
  return (
    permissionSet.pack_ref ??
    permissionSet.ref.split(".")[0] ??
    permissionSet.ref
  );
}

function getPermissionSetLabel(permissionSet: PermissionSetDescriptor): string {
  return permissionSet.label?.trim() || permissionSet.ref;
}

function getPermissionSetHref(ref: string): string {
  return `/access-control/permission-sets/${encodeURIComponent(ref)}`;
}

function sortPermissionSets(
  left: PermissionSetDescriptor,
  right: PermissionSetDescriptor,
): number {
  return (
    getPermissionSetPackName(left).localeCompare(
      getPermissionSetPackName(right),
    ) ||
    getPermissionSetLabel(left).localeCompare(getPermissionSetLabel(right)) ||
    left.ref.localeCompare(right.ref)
  );
}

function sortEffectiveGrantRows(
  left: EffectiveGrantRow,
  right: EffectiveGrantRow,
): number {
  return (
    left.grant.resource.localeCompare(right.grant.resource) ||
    left.grant.actions.join(",").localeCompare(right.grant.actions.join(",")) ||
    getGrantKey(left.grant).localeCompare(getGrantKey(right.grant))
  );
}

function PermissionSetLink({
  permissionSet,
}: {
  permissionSet: PermissionSetInfo;
}) {
  const href = getPermissionSetHref(permissionSet.ref);
  const packName = getPermissionSetPackName(permissionSet);
  const label = getPermissionSetLabel(permissionSet);

  return (
    <div className="min-w-0">
      <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1 text-sm">
        <Link
          to={href}
          className="font-medium text-blue-700 hover:text-blue-800 hover:underline"
        >
          {packName}
        </Link>
        <span className="text-gray-300">/</span>
        <Link
          to={href}
          className="font-medium text-gray-900 hover:text-blue-700 hover:underline"
        >
          {label}
        </Link>
      </div>
      <div className="mt-0.5 truncate text-xs font-mono text-gray-400">
        {permissionSet.ref}
      </div>
    </div>
  );
}

export default function IdentityDetailPage() {
  const { id: idParam } = useParams<{ id: string }>();
  const id = Number(idParam) || 0;

  const { data: rawData, isLoading, error } = useIdentity(id);
  const { data: permissionSets } = usePermissionSets();

  const createRoleMutation = useCreateIdentityRoleAssignment();
  const deleteRoleMutation = useDeleteIdentityRoleAssignment();
  const createPermMutation = useCreatePermissionAssignment();
  const deletePermMutation = useDeletePermissionAssignment();
  const freezeMutation = useFreezeIdentity();
  const unfreezeMutation = useUnfreezeIdentity();
  const { data: integrationTokens = [] } = useIntegrationTokens(id);
  const createTokenMutation = useCreateIntegrationToken();
  const revokeTokenMutation = useRevokeIntegrationToken();
  const deleteTokenMutation = useDeleteIntegrationToken();

  const [showAddRole, setShowAddRole] = useState(false);
  const [newRole, setNewRole] = useState("");
  const [showAssignPerm, setShowAssignPerm] = useState(false);
  const [selectedPermSetRef, setSelectedPermSetRef] = useState("");
  const [permSetSearch, setPermSetSearch] = useState("");
  const [showRolesWithoutPermissions, setShowRolesWithoutPermissions] =
    useState(false);
  const [showCreateToken, setShowCreateToken] = useState(false);
  const [tokenLabel, setTokenLabel] = useState("");
  const [tokenDescription, setTokenDescription] = useState("");
  const [tokenExpiresAt, setTokenExpiresAt] = useState("");
  const [newTokenSecret, setNewTokenSecret] = useState<string | null>(null);

  const identity = (rawData as unknown as { data: IdentityDetail } | undefined)
    ?.data;

  const handleAddRole = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newRole.trim()) return;
    try {
      await createRoleMutation.mutateAsync({
        identityId: id,
        role: newRole.trim(),
      });
      setNewRole("");
      setShowAddRole(false);
    } catch (err) {
      console.error("Failed to add role:", err);
    }
  };

  const handleDeleteRole = async (assignmentId: number, role: string) => {
    if (window.confirm('Remove role "' + role + '" from this identity?')) {
      try {
        await deleteRoleMutation.mutateAsync(assignmentId);
      } catch (err) {
        console.error("Failed to delete role assignment:", err);
      }
    }
  };

  const handleAssignPermission = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedPermSetRef) return;
    try {
      await createPermMutation.mutateAsync({
        identity_id: id,
        permission_set_ref: selectedPermSetRef,
      });
      setSelectedPermSetRef("");
      setPermSetSearch("");
      setShowAssignPerm(false);
    } catch (err) {
      console.error("Failed to assign permission set:", err);
    }
  };

  const handleDeletePermission = async (assignmentId: number, ref: string) => {
    if (
      window.confirm('Remove permission set "' + ref + '" from this identity?')
    ) {
      try {
        await deletePermMutation.mutateAsync(assignmentId);
      } catch (err) {
        console.error("Failed to remove permission assignment:", err);
      }
    }
  };

  const handleToggleFreeze = async () => {
    if (!identity) return;
    const action = identity.frozen ? "unfreeze" : "freeze";
    if (
      !window.confirm(
        "Are you sure you want to " +
          action +
          ' identity "' +
          identity.login +
          '"?',
      )
    )
      return;
    try {
      if (identity.frozen) {
        await unfreezeMutation.mutateAsync(id);
      } else {
        await freezeMutation.mutateAsync(id);
      }
    } catch (err) {
      console.error("Failed to " + action + " identity:", err);
    }
  };

  const handleCreateIntegrationToken = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!tokenLabel.trim()) return;
    const response = await createTokenMutation.mutateAsync({
      identityId: id,
      data: {
        label: tokenLabel.trim(),
        description: tokenDescription.trim() || null,
        expires_at: tokenExpiresAt
          ? new Date(tokenExpiresAt).toISOString()
          : null,
      },
    });
    setNewTokenSecret(response.token);
    setTokenLabel("");
    setTokenDescription("");
    setTokenExpiresAt("");
    setShowCreateToken(false);
  };

  const handleRevokeIntegrationToken = async (token: IntegrationToken) => {
    if (!window.confirm(`Revoke integration token "${token.label}"?`)) return;
    await revokeTokenMutation.mutateAsync({
      identityId: id,
      tokenId: token.id,
      reason: "Revoked from web UI",
    });
  };

  const handleDeleteIntegrationToken = async (token: IntegrationToken) => {
    if (
      !window.confirm(`Delete integration token metadata for "${token.label}"?`)
    )
      return;
    await deleteTokenMutation.mutateAsync({
      identityId: id,
      tokenId: token.id,
    });
  };

  const copyNewToken = async () => {
    if (!newTokenSecret) return;
    await navigator.clipboard.writeText(newTokenSecret);
  };

  const formatDate = (dateString: string) =>
    new Date(dateString).toLocaleString();

  const permissionSetByRef = useMemo(
    () =>
      new Map<string, PermissionSetSummary>(
        (permissionSets ?? []).map((permissionSet) => [
          permissionSet.ref,
          permissionSet,
        ]),
      ),
    [permissionSets],
  );

  const resolvePermissionSet = useMemo(
    () =>
      (ref: string): PermissionSetInfo => {
        const permissionSet = permissionSetByRef.get(ref);
        return {
          ref,
          label: permissionSet?.label,
          pack_ref: permissionSet?.pack_ref,
          grants: permissionSet ? parseGrants(permissionSet.grants) : [],
        };
      },
    [permissionSetByRef],
  );

  const rolePermissionSets = useMemo(() => {
    const byRole = new Map<string, PermissionSetSummary[]>();

    for (const permissionSet of permissionSets ?? []) {
      for (const roleAssignment of permissionSet.roles ?? []) {
        const matches = byRole.get(roleAssignment.role) ?? [];
        matches.push(permissionSet);
        byRole.set(roleAssignment.role, matches);
      }
    }

    for (const matches of byRole.values()) {
      matches.sort(sortPermissionSets);
    }

    return byRole;
  }, [permissionSets]);

  const rolesWithPermissions = useMemo(
    () =>
      (identity?.roles ?? []).filter(
        (assignment) =>
          (rolePermissionSets.get(assignment.role)?.length ?? 0) > 0,
      ),
    [identity?.roles, rolePermissionSets],
  );

  const rolesWithoutPermissions = useMemo(
    () =>
      (identity?.roles ?? []).filter(
        (assignment) =>
          (rolePermissionSets.get(assignment.role)?.length ?? 0) === 0,
      ),
    [identity?.roles, rolePermissionSets],
  );

  const directPermissionSetRefs = useMemo(
    () =>
      (identity?.direct_permissions ?? []).map(
        (assignment) => assignment.permission_set_ref,
      ),
    [identity?.direct_permissions],
  );

  const assignedRoleNames = useMemo(
    () => (identity?.roles ?? []).map((assignment) => assignment.role),
    [identity?.roles],
  );

  const effectivePermissionSets = useMemo(() => {
    const byRef = new Map<
      string,
      {
        permissionSet: PermissionSetInfo;
        direct: boolean;
        viaRoles: Set<string>;
      }
    >();

    for (const permissionSetRef of directPermissionSetRefs) {
      const existing = byRef.get(permissionSetRef);
      if (existing) {
        existing.direct = true;
      } else {
        byRef.set(permissionSetRef, {
          permissionSet: resolvePermissionSet(permissionSetRef),
          direct: true,
          viaRoles: new Set<string>(),
        });
      }
    }

    for (const roleName of assignedRoleNames) {
      for (const permissionSet of rolePermissionSets.get(roleName) ?? []) {
        const existing = byRef.get(permissionSet.ref);
        if (existing) {
          existing.viaRoles.add(roleName);
        } else {
          byRef.set(permissionSet.ref, {
            permissionSet: resolvePermissionSet(permissionSet.ref),
            direct: false,
            viaRoles: new Set<string>([roleName]),
          });
        }
      }
    }

    return Array.from(byRef.values())
      .map(({ permissionSet, direct, viaRoles }) => ({
        ...permissionSet,
        direct,
        viaRoles: Array.from(viaRoles).sort((left, right) =>
          left.localeCompare(right),
        ),
      }))
      .sort(sortPermissionSets);
  }, [
    assignedRoleNames,
    directPermissionSetRefs,
    resolvePermissionSet,
    rolePermissionSets,
  ]);

  const effectiveGrantRows = useMemo(() => {
    const grantsByKey = new Map<
      string,
      {
        grant: ParsedGrant;
        permissionSets: Map<string, EffectivePermissionSet>;
      }
    >();

    for (const permissionSet of effectivePermissionSets) {
      for (const grant of permissionSet.grants) {
        const key = getGrantKey(grant);
        const existing =
          grantsByKey.get(key) ??
          (() => {
            const created = {
              grant,
              permissionSets: new Map<string, EffectivePermissionSet>(),
            };
            grantsByKey.set(key, created);
            return created;
          })();

        existing.permissionSets.set(permissionSet.ref, permissionSet);
      }
    }

    return Array.from(grantsByKey.values())
      .map(({ grant, permissionSets: permissionSetMap }) => ({
        grant,
        permissionSets: Array.from(permissionSetMap.values()).sort(
          sortPermissionSets,
        ),
      }))
      .sort(sortEffectiveGrantRows);
  }, [effectivePermissionSets]);

  const assignedPermSetRefs = new Set(
    identity?.direct_permissions?.map((p) => p.permission_set_ref) ?? [],
  );
  const availablePermSets = (permissionSets ?? []).filter(
    (ps) => !assignedPermSetRefs.has(ps.ref),
  );
  const filteredAvailablePermSets = permSetSearch.trim()
    ? availablePermSets.filter(
        (ps) =>
          ps.ref.toLowerCase().includes(permSetSearch.toLowerCase()) ||
          (ps.label ?? "").toLowerCase().includes(permSetSearch.toLowerCase()),
      )
    : availablePermSets;

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="text-center">
            <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600 mx-auto"></div>
            <p className="mt-3 text-sm text-gray-500">Loading identity...</p>
          </div>
        </div>
      </div>
    );
  }

  if (error || !identity) {
    return (
      <div className="p-6">
        <Link
          to="/access-control"
          className="inline-flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800 mb-6"
        >
          <ArrowLeft className="w-4 h-4" /> Back to Access Control
        </Link>
        <div className="bg-white rounded-lg shadow p-12 text-center">
          <Shield className="mx-auto h-12 w-12 text-gray-400" />
          <p className="mt-4 text-red-600">Failed to load identity</p>
          <p className="text-sm text-gray-500 mt-1">
            {error instanceof Error ? error.message : "Identity not found"}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-6">
      <Link
        to="/access-control"
        className="inline-flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800 mb-6"
      >
        <ArrowLeft className="w-4 h-4" /> Back to Access Control
      </Link>

      {/* Header */}
      <div className="bg-white rounded-lg shadow p-6 mb-6">
        <div className="flex items-start justify-between">
          <div className="flex items-center gap-4">
            <div className="bg-blue-100 p-3 rounded-full">
              <User className="w-6 h-6 text-blue-600" />
            </div>
            <div>
              <div className="flex items-center gap-3">
                <h1 className="text-2xl font-bold text-gray-900">
                  {identity.login}
                </h1>
                {identity.frozen && (
                  <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-semibold bg-blue-100 text-blue-800">
                    <Snowflake className="w-3 h-3" /> Frozen
                  </span>
                )}
              </div>
              {identity.display_name && (
                <p className="text-gray-600 mt-1">{identity.display_name}</p>
              )}
              <p className="text-sm text-gray-400 mt-1">ID: {identity.id}</p>
            </div>
          </div>
          <button
            onClick={handleToggleFreeze}
            disabled={freezeMutation.isPending || unfreezeMutation.isPending}
            className={
              "inline-flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium transition-colors disabled:opacity-50 " +
              (identity.frozen
                ? "bg-green-50 text-green-700 hover:bg-green-100 border border-green-200"
                : "bg-blue-50 text-blue-700 hover:bg-blue-100 border border-blue-200")
            }
          >
            {identity.frozen ? (
              <>
                <Sun className="w-4 h-4" /> Unfreeze
              </>
            ) : (
              <>
                <Snowflake className="w-4 h-4" /> Freeze
              </>
            )}
          </button>
        </div>
      </div>

      <div className="grid gap-6 xl:grid-cols-2 xl:items-start">
        <div className="space-y-6">
          {/* Roles Section */}
          <div className="bg-white rounded-lg shadow p-6">
            <div className="flex items-center justify-between mb-4">
              <div className="flex items-center gap-2">
                <Tag className="w-5 h-5 text-violet-500" />
                <h2 className="text-lg font-semibold text-gray-900">
                  Role Assignments
                </h2>
                <span className="text-sm text-gray-500">
                  ({identity.roles?.length || 0})
                </span>
              </div>
              <button
                onClick={() => setShowAddRole(!showAddRole)}
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm bg-violet-600 text-white rounded-lg hover:bg-violet-700 transition-colors"
              >
                <Plus className="w-4 h-4" /> Add Role
              </button>
            </div>

            {showAddRole && (
              <form
                onSubmit={handleAddRole}
                className="flex items-center gap-3 mb-4 p-3 bg-gray-50 rounded-lg"
              >
                <input
                  type="text"
                  value={newRole}
                  onChange={(e) => setNewRole(e.target.value)}
                  placeholder="Role name (e.g. admin, operator, viewer)"
                  className="flex-1 px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-violet-500 text-sm"
                  autoFocus
                />
                <button
                  type="submit"
                  disabled={!newRole.trim() || createRoleMutation.isPending}
                  className="px-4 py-2 bg-violet-600 text-white rounded-lg hover:bg-violet-700 disabled:opacity-50 text-sm transition-colors"
                >
                  {createRoleMutation.isPending ? "Adding..." : "Add"}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setShowAddRole(false);
                    setNewRole("");
                  }}
                  className="px-4 py-2 text-gray-600 hover:text-gray-900 text-sm"
                >
                  Cancel
                </button>
              </form>
            )}

            {identity.roles && identity.roles.length > 0 ? (
              <div className="space-y-4">
                {rolesWithPermissions.length > 0 ? (
                  <div className="space-y-3">
                    {rolesWithPermissions.map((assignment) => {
                      const grantedPermissionSets =
                        rolePermissionSets
                          .get(assignment.role)
                          ?.map((permissionSet) => ({
                            ref: permissionSet.ref,
                            label: permissionSet.label,
                            pack_ref: permissionSet.pack_ref,
                            grants: parseGrants(permissionSet.grants),
                          })) ?? [];

                      return (
                        <div
                          key={assignment.id}
                          className="rounded-lg border border-violet-100 bg-violet-50/40 p-4"
                        >
                          <div className="flex items-start justify-between gap-4">
                            <div className="min-w-0">
                              <div className="flex flex-wrap items-center gap-2">
                                <span className="inline-flex items-center px-2.5 py-1 rounded-full text-sm font-medium bg-purple-100 text-purple-800">
                                  {assignment.role}
                                </span>
                                <span className="text-xs text-gray-500">
                                  Source: {assignment.source}
                                </span>
                                {assignment.managed && (
                                  <span className="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-amber-100 text-amber-800">
                                    Managed
                                  </span>
                                )}
                                <span className="text-xs text-gray-400">
                                  {formatDate(assignment.created)}
                                </span>
                              </div>
                              <div className="mt-3 space-y-2">
                                <p className="text-xs font-medium uppercase tracking-wide text-violet-700">
                                  Grants permission sets
                                </p>
                                {grantedPermissionSets.map((permissionSet) => (
                                  <div
                                    key={`${assignment.id}-${permissionSet.ref}`}
                                    className="rounded-md border border-violet-200 bg-white px-3 py-2"
                                  >
                                    <PermissionSetLink
                                      permissionSet={permissionSet}
                                    />
                                  </div>
                                ))}
                              </div>
                            </div>
                            {!assignment.managed && (
                              <button
                                onClick={() =>
                                  handleDeleteRole(
                                    assignment.id,
                                    assignment.role,
                                  )
                                }
                                className="text-red-400 hover:text-red-600 p-1"
                                title="Remove role"
                              >
                                <Trash2 className="w-4 h-4" />
                              </button>
                            )}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                ) : (
                  <div className="rounded-lg border border-dashed border-violet-200 bg-violet-50/40 px-4 py-5 text-sm text-violet-800">
                    This identity has assigned roles, but none of them grant a
                    permission set.
                  </div>
                )}

                {rolesWithoutPermissions.length > 0 && (
                  <div className="rounded-lg border border-gray-200">
                    <button
                      type="button"
                      onClick={() =>
                        setShowRolesWithoutPermissions(
                          !showRolesWithoutPermissions,
                        )
                      }
                      className="flex w-full items-center justify-between px-4 py-3 text-left"
                    >
                      <div>
                        <p className="text-sm font-medium text-gray-900">
                          Roles without permission sets
                        </p>
                      </div>
                      <span className="inline-flex items-center gap-2 text-sm text-gray-600">
                        {rolesWithoutPermissions.length}
                        {showRolesWithoutPermissions ? (
                          <ChevronDown className="h-4 w-4" />
                        ) : (
                          <ChevronRight className="h-4 w-4" />
                        )}
                      </span>
                    </button>
                    {showRolesWithoutPermissions && (
                      <div className="divide-y divide-gray-100 border-t border-gray-200">
                        {rolesWithoutPermissions.map((assignment) => (
                          <div
                            key={assignment.id}
                            className="flex items-center justify-between gap-4 px-4 py-3"
                          >
                            <div className="min-w-0">
                              <div className="flex flex-wrap items-center gap-2">
                                <span className="inline-flex items-center px-2.5 py-1 rounded-full text-sm font-medium bg-gray-100 text-gray-700">
                                  {assignment.role}
                                </span>
                                <span className="text-xs text-gray-500">
                                  Source: {assignment.source}
                                </span>
                                {assignment.managed && (
                                  <span className="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-amber-100 text-amber-800">
                                    Managed
                                  </span>
                                )}
                                <span className="text-xs text-gray-400">
                                  {formatDate(assignment.created)}
                                </span>
                              </div>
                            </div>
                            {!assignment.managed && (
                              <button
                                onClick={() =>
                                  handleDeleteRole(
                                    assignment.id,
                                    assignment.role,
                                  )
                                }
                                className="text-red-400 hover:text-red-600 p-1"
                                title="Remove role"
                              >
                                <Trash2 className="w-4 h-4" />
                              </button>
                            )}
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            ) : (
              <div className="text-center py-6">
                <Tag className="mx-auto h-8 w-8 text-gray-300" />
                <p className="mt-2 text-sm text-gray-500">No roles assigned</p>
              </div>
            )}
          </div>

          {/* Direct Permission Sets Section */}
          <div className="bg-white rounded-lg shadow p-6">
            <div className="flex items-center justify-between mb-4">
              <div className="flex items-center gap-2">
                <ShieldCheck className="w-5 h-5 text-indigo-500" />
                <h2 className="text-lg font-semibold text-gray-900">
                  Direct Permission Sets
                </h2>
                <span className="text-sm text-gray-500">
                  ({identity.direct_permissions?.length || 0})
                </span>
              </div>
              <button
                onClick={() => setShowAssignPerm(!showAssignPerm)}
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm bg-indigo-600 text-white rounded-lg hover:bg-indigo-700 transition-colors"
              >
                <Plus className="w-4 h-4" /> Assign Permission Set
              </button>
            </div>

            {showAssignPerm && (
              <form
                onSubmit={handleAssignPermission}
                className="mb-4 p-3 bg-gray-50 rounded-lg space-y-2"
              >
                {selectedPermSetRef ? (
                  <div className="flex items-center gap-2">
                    <div className="flex items-center gap-2 px-3 py-1.5 bg-indigo-100 text-indigo-800 rounded-md text-sm font-mono flex-1">
                      <Shield className="w-3.5 h-3.5 flex-shrink-0" />
                      <span className="truncate">{selectedPermSetRef}</span>
                      <button
                        type="button"
                        onClick={() => {
                          setSelectedPermSetRef("");
                          setPermSetSearch("");
                        }}
                        className="ml-auto text-indigo-500 hover:text-indigo-700"
                      >
                        &#x2715;
                      </button>
                    </div>
                    <button
                      type="submit"
                      disabled={createPermMutation.isPending}
                      className="px-4 py-2 bg-indigo-600 text-white rounded-lg hover:bg-indigo-700 disabled:opacity-50 text-sm transition-colors whitespace-nowrap"
                    >
                      {createPermMutation.isPending ? "Assigning..." : "Assign"}
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setShowAssignPerm(false);
                        setSelectedPermSetRef("");
                        setPermSetSearch("");
                      }}
                      className="px-3 py-2 text-gray-600 hover:text-gray-900 text-sm"
                    >
                      Cancel
                    </button>
                  </div>
                ) : (
                  <div className="relative">
                    <div className="flex items-center gap-2">
                      <div className="relative flex-1">
                        <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400 pointer-events-none" />
                        <input
                          type="text"
                          value={permSetSearch}
                          onChange={(e) => setPermSetSearch(e.target.value)}
                          placeholder="Search permission sets by ref or label..."
                          className="w-full pl-9 pr-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-indigo-500 text-sm"
                          autoFocus
                        />
                      </div>
                      <button
                        type="button"
                        onClick={() => {
                          setShowAssignPerm(false);
                          setPermSetSearch("");
                        }}
                        className="px-3 py-2 text-gray-600 hover:text-gray-900 text-sm"
                      >
                        Cancel
                      </button>
                    </div>
                    {permSetSearch.trim() && (
                      <div className="mt-1 bg-white border border-gray-200 rounded-lg shadow-lg max-h-52 overflow-y-auto">
                        {filteredAvailablePermSets.length === 0 ? (
                          <div className="px-4 py-3 text-sm text-gray-500">
                            No matching permission sets
                          </div>
                        ) : (
                          filteredAvailablePermSets.map((ps) => (
                            <button
                              key={ps.ref}
                              type="button"
                              onClick={() => {
                                setSelectedPermSetRef(ps.ref);
                                setPermSetSearch("");
                              }}
                              className="w-full flex items-start gap-3 px-4 py-2.5 text-left hover:bg-indigo-50 transition-colors"
                            >
                              <Shield className="w-4 h-4 text-indigo-400 flex-shrink-0 mt-0.5" />
                              <div className="min-w-0">
                                <div className="text-sm font-mono font-medium text-gray-900 truncate">
                                  {ps.ref}
                                </div>
                                {ps.label && (
                                  <div className="text-xs text-gray-500 truncate">
                                    {ps.label}
                                  </div>
                                )}
                              </div>
                            </button>
                          ))
                        )}
                      </div>
                    )}
                  </div>
                )}
              </form>
            )}

            {identity.direct_permissions &&
            identity.direct_permissions.length > 0 ? (
              <div className="divide-y divide-gray-100">
                {identity.direct_permissions.map((assignment) => {
                  const permissionSet = resolvePermissionSet(
                    assignment.permission_set_ref,
                  );

                  return (
                    <div
                      key={assignment.id}
                      className="flex items-start justify-between gap-4 py-3"
                    >
                      <div className="flex items-start gap-3 min-w-0">
                        <Shield className="w-4 h-4 text-indigo-400 mt-0.5" />
                        <div className="min-w-0">
                          <PermissionSetLink permissionSet={permissionSet} />
                          <div className="mt-1 text-xs text-gray-400">
                            Assigned {formatDate(assignment.created)}
                          </div>
                        </div>
                      </div>
                      <button
                        onClick={() =>
                          handleDeletePermission(
                            assignment.id,
                            assignment.permission_set_ref,
                          )
                        }
                        className="text-red-400 hover:text-red-600 p-1"
                        title="Remove permission set"
                      >
                        <Trash2 className="w-4 h-4" />
                      </button>
                    </div>
                  );
                })}
              </div>
            ) : (
              <div className="text-center py-6">
                <ShieldCheck className="mx-auto h-8 w-8 text-gray-300" />
                <p className="mt-2 text-sm text-gray-500">
                  No direct permission sets assigned
                </p>
                <p className="text-xs text-gray-400 mt-1">
                  Permission sets can also be inherited through roles
                </p>
              </div>
            )}
          </div>

          {/* Integration Tokens Section */}
          <div className="bg-white rounded-lg shadow p-6">
            <div className="flex items-center justify-between mb-4">
              <div className="flex items-center gap-2">
                <KeyRound className="w-5 h-5 text-amber-500" />
                <h2 className="text-lg font-semibold text-gray-900">
                  Integration Tokens
                </h2>
                <span className="text-sm text-gray-500">
                  ({integrationTokens.length})
                </span>
              </div>
              <button
                onClick={() => setShowCreateToken(!showCreateToken)}
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm bg-amber-600 text-white rounded-lg hover:bg-amber-700 transition-colors"
              >
                <Plus className="w-4 h-4" /> Create Token
              </button>
            </div>

            {newTokenSecret && (
              <div className="mb-4 rounded-lg border border-amber-200 bg-amber-50 p-4">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <p className="text-sm font-medium text-amber-900">
                      Copy this token now. It will not be shown again.
                    </p>
                    <code className="mt-2 block break-all rounded bg-white px-3 py-2 text-xs text-amber-900 border border-amber-200">
                      {newTokenSecret}
                    </code>
                  </div>
                  <button
                    type="button"
                    onClick={copyNewToken}
                    className="inline-flex items-center gap-1 rounded-md border border-amber-300 px-2 py-1 text-xs font-medium text-amber-800 hover:bg-amber-100"
                  >
                    <Copy className="w-3 h-3" /> Copy
                  </button>
                </div>
                <button
                  type="button"
                  onClick={() => setNewTokenSecret(null)}
                  className="mt-3 text-xs text-amber-800 hover:text-amber-950"
                >
                  Dismiss
                </button>
              </div>
            )}

            {showCreateToken && (
              <form
                onSubmit={handleCreateIntegrationToken}
                className="mb-4 p-3 bg-gray-50 rounded-lg space-y-3"
              >
                <div>
                  <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                    Label
                  </label>
                  <input
                    value={tokenLabel}
                    onChange={(e) => setTokenLabel(e.target.value)}
                    required
                    maxLength={255}
                    placeholder="CI deploy bot"
                    className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-amber-500"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                    Description
                  </label>
                  <textarea
                    value={tokenDescription}
                    onChange={(e) => setTokenDescription(e.target.value)}
                    maxLength={2000}
                    rows={2}
                    placeholder="What this integration token is used for"
                    className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-amber-500"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                    Expires At
                  </label>
                  <input
                    type="datetime-local"
                    value={tokenExpiresAt}
                    onChange={(e) => setTokenExpiresAt(e.target.value)}
                    className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-amber-500"
                  />
                </div>
                <div className="flex justify-end gap-2">
                  <button
                    type="button"
                    onClick={() => setShowCreateToken(false)}
                    className="px-3 py-2 text-sm text-gray-600 hover:text-gray-900"
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    disabled={
                      !tokenLabel.trim() || createTokenMutation.isPending
                    }
                    className="px-4 py-2 text-sm text-white bg-amber-600 rounded-lg hover:bg-amber-700 disabled:opacity-50"
                  >
                    {createTokenMutation.isPending ? "Creating..." : "Create"}
                  </button>
                </div>
              </form>
            )}

            {integrationTokens.length > 0 ? (
              <div className="divide-y divide-gray-100">
                {integrationTokens.map((token) => (
                  <div
                    key={token.id}
                    className="flex items-center justify-between py-3 gap-4"
                  >
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-medium text-gray-900 truncate">
                          {token.label}
                        </span>
                        <span
                          className={
                            "inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium " +
                            (token.active
                              ? "bg-green-100 text-green-800"
                              : "bg-gray-100 text-gray-600")
                          }
                        >
                          {token.active ? "Active" : "Inactive"}
                        </span>
                      </div>
                      <div className="mt-1 text-xs text-gray-500">
                        <span className="font-mono">
                          {token.token_prefix}...{token.token_suffix}
                        </span>
                        <span className="mx-2">·</span>
                        <span>Created {formatDate(token.created)}</span>
                        <span className="mx-2">·</span>
                        <span>
                          Expires{" "}
                          {token.expires_at
                            ? formatDate(token.expires_at)
                            : "never"}
                        </span>
                        <span className="mx-2">·</span>
                        <span>
                          Last used{" "}
                          {token.last_used_at
                            ? formatDate(token.last_used_at)
                            : "never"}
                        </span>
                      </div>
                      {token.description && (
                        <p className="mt-1 text-xs text-gray-500 truncate">
                          {token.description}
                        </p>
                      )}
                    </div>
                    <div className="flex items-center gap-2 flex-shrink-0">
                      {token.active && (
                        <button
                          onClick={() => handleRevokeIntegrationToken(token)}
                          disabled={revokeTokenMutation.isPending}
                          className="px-3 py-1.5 text-xs font-medium text-amber-700 bg-amber-50 border border-amber-200 rounded-md hover:bg-amber-100 disabled:opacity-50"
                        >
                          Revoke
                        </button>
                      )}
                      <button
                        onClick={() => handleDeleteIntegrationToken(token)}
                        disabled={deleteTokenMutation.isPending}
                        className="text-red-400 hover:text-red-600 p-1 disabled:opacity-50"
                        title="Delete token metadata"
                      >
                        <Trash2 className="w-4 h-4" />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <div className="text-center py-6">
                <KeyRound className="mx-auto h-8 w-8 text-gray-300" />
                <p className="mt-2 text-sm text-gray-500">
                  No integration tokens
                </p>
                <p className="text-xs text-gray-400 mt-1">
                  Create a token to enable passwordless API login for an
                  integration.
                </p>
              </div>
            )}
          </div>

          {/* Attributes Section */}
          <div className="bg-white rounded-lg shadow p-6">
            <div className="flex items-center gap-2 mb-4">
              <FileJson className="w-5 h-5 text-gray-500" />
              <h2 className="text-lg font-semibold text-gray-900">
                Attributes
              </h2>
            </div>
            <pre className="bg-gray-50 border border-gray-200 rounded-lg p-4 text-sm text-gray-800 overflow-x-auto max-h-64 overflow-y-auto font-mono leading-relaxed">
              {JSON.stringify(identity.attributes, null, 2)}
            </pre>
          </div>
        </div>

        <div className="xl:sticky xl:top-6 xl:h-[calc(100vh-15rem)]">
          <div className="bg-white rounded-lg shadow p-6 xl:flex xl:h-full xl:min-h-0 xl:flex-col">
            <div className="mb-4">
              <div className="flex items-center gap-2">
                <Shield className="w-5 h-5 text-indigo-500" />
                <h2 className="text-lg font-semibold text-gray-900">
                  Effective Permissions
                </h2>
              </div>
              <p className="mt-1 text-sm text-gray-500">
                Aggregated across direct assignments and role-derived permission
                sets, with duplicate grants collapsed.
              </p>
            </div>

            <GrantsView
              grants={effectiveGrantRows.map((row) => row.grant)}
              emptyStateTitle="No effective permissions"
              emptyStateDescription="Assign a permission set directly or through a role to grant access."
              sourceColumnTitle="Granted by"
              scrollClassName="overflow-y-auto xl:flex-1 xl:min-h-0"
              renderSource={(_, index) => {
                const row = effectiveGrantRows[index];

                if (!row) {
                  return null;
                }

                return (
                  <div className="space-y-2">
                    {row.permissionSets.map((permissionSet) => (
                      <div
                        key={`${row.grant.resource}-${permissionSet.ref}`}
                        className="rounded-md border border-gray-200 bg-gray-50 px-3 py-2"
                      >
                        <PermissionSetLink permissionSet={permissionSet} />
                        <div className="mt-1 flex flex-wrap gap-1.5 text-xs">
                          {permissionSet.direct && (
                            <span className="inline-flex items-center rounded-full bg-indigo-100 px-2 py-0.5 font-medium text-indigo-800">
                              Direct
                            </span>
                          )}
                          {permissionSet.viaRoles.map((role) => (
                            <span
                              key={`${permissionSet.ref}-${role}`}
                              className="inline-flex items-center rounded-full bg-violet-100 px-2 py-0.5 font-medium text-violet-800"
                            >
                              {role}
                            </span>
                          ))}
                        </div>
                      </div>
                    ))}
                  </div>
                );
              }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
