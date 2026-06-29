import { useState } from "react";
import { useSearchParams, Link } from "react-router-dom";
import {
  Shield,
  User,
  Users,
  Plus,
  Search,
  ShieldCheck,
  X,
  Snowflake,
  Sun,
  Package,
  Tag,
} from "lucide-react";
import {
  useIdentities,
  useCreateIdentity,
  usePermissionSets,
  useFreezeIdentity,
  useUnfreezeIdentity,
} from "@/hooks/usePermissions";
import Pagination from "@/components/executions/Pagination";

// The backend IdentitySummary includes `frozen` and `roles` but the generated client type doesn't declare them
interface IdentityRow {
  id: number;
  login: string;
  display_name?: string | null;
  frozen?: boolean;
  roles?: string[];
  attributes: Record<string, unknown>;
}

// The backend PermissionSetSummary includes `roles` but the generated client type doesn't declare it
interface PermissionSetRow {
  id: number;
  ref: string;
  pack_ref?: string | null;
  label?: string | null;
  description?: string | null;
  grants: unknown;
  roles?: Array<{
    id: number;
    permission_set_id: number;
    permission_set_ref?: string | null;
    role: string;
    created: string;
  }>;
}

function CreateIdentityModal({ onClose }: { onClose: () => void }) {
  const [login, setLogin] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const createIdentity = useCreateIdentity();

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      await createIdentity.mutateAsync({
        login,
        display_name: displayName || undefined,
        password: password || undefined,
      });
      onClose();
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to create identity",
      );
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg shadow-xl w-full max-w-md mx-4">
        <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200">
          <h3 className="text-lg font-semibold text-gray-900">
            Create Identity
          </h3>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <form onSubmit={handleSubmit} className="p-6 space-y-4">
          {error && (
            <div className="p-3 bg-red-50 border border-red-200 rounded-lg text-sm text-red-700">
              {error}
            </div>
          )}
          <div>
            <label
              htmlFor="login"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Login <span className="text-red-500">*</span>
            </label>
            <input
              id="login"
              type="text"
              value={login}
              onChange={(e) => setLogin(e.target.value)}
              required
              minLength={3}
              maxLength={255}
              placeholder="e.g. jane.doe"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
          <div>
            <label
              htmlFor="display-name"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Display Name
            </label>
            <input
              id="display-name"
              type="text"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="e.g. Jane Doe"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
          <div>
            <label
              htmlFor="password"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Password
            </label>
            <input
              id="password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              minLength={8}
              maxLength={128}
              placeholder="Min 8 characters (optional)"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>
          <div className="flex justify-end gap-3 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-sm text-gray-700 bg-gray-100 rounded-lg hover:bg-gray-200 transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={createIdentity.isPending || !login}
              className="px-4 py-2 text-sm text-white bg-blue-600 rounded-lg hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {createIdentity.isPending ? "Creating\u2026" : "Create Identity"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

function RoleBadges({ roles }: { roles: string[] }) {
  const [showTooltip, setShowTooltip] = useState(false);
  const visible = roles.slice(0, 3);
  const remaining = roles.length - visible.length;

  if (roles.length === 0) {
    return <span className="text-xs text-gray-400">None</span>;
  }

  return (
    <div className="flex items-center gap-1 flex-wrap">
      {visible.map((role) => (
        <span
          key={role}
          className="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-purple-100 text-purple-800"
        >
          {role}
        </span>
      ))}
      {remaining > 0 && (
        <div
          className="relative"
          onMouseEnter={() => setShowTooltip(true)}
          onMouseLeave={() => setShowTooltip(false)}
        >
          <span className="text-xs text-gray-500 cursor-default">
            and {remaining} more {remaining === 1 ? "role" : "roles"}
          </span>
          {showTooltip && (
            <div className="absolute bottom-full left-0 mb-2 z-20 bg-gray-900 text-white text-xs rounded-lg shadow-lg p-3 whitespace-nowrap">
              <p className="font-medium mb-1">All roles:</p>
              <ul className="space-y-0.5">
                {roles.map((role) => (
                  <li key={role}>{role}</li>
                ))}
              </ul>
              <div className="absolute top-full left-4 w-2 h-2 bg-gray-900 rotate-45 -mt-1" />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function IdentitiesTab() {
  const [page, setPage] = useState(1);
  const [searchTerm, setSearchTerm] = useState("");
  const [showCreateModal, setShowCreateModal] = useState(false);
  const pageSize = 20;

  const { data, isLoading, error } = useIdentities({ page, pageSize });
  const freezeIdentity = useFreezeIdentity();
  const unfreezeIdentity = useUnfreezeIdentity();

  const identities: IdentityRow[] = (data?.items as IdentityRow[]) || [];
  const total = data?.pagination?.total_items || 0;
  const filteredIdentities = searchTerm
    ? identities.filter((i) => {
        const q = searchTerm.toLowerCase();
        return (
          i.login.toLowerCase().includes(q) ||
          (i.display_name || "").toLowerCase().includes(q) ||
          (i.roles || []).some((r) => r.toLowerCase().includes(q))
        );
      })
    : identities;

  const handleToggleFreeze = async (identity: IdentityRow) => {
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
        await unfreezeIdentity.mutateAsync(identity.id);
      } else {
        await freezeIdentity.mutateAsync(identity.id);
      }
    } catch (err) {
      console.error("Failed to " + action + " identity:", err);
    }
  };

  return (
    <div className="pb-28">
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div>
            <h2 className="text-xl font-semibold text-gray-900 flex items-center gap-2">
              <Users className="w-5 h-5 text-blue-600" />
              Identities
            </h2>
            <p className="mt-1 text-sm text-gray-600">
              Manage user and service identities, their roles, and permission
              assignments
            </p>
          </div>
          <button
            onClick={() => setShowCreateModal(true)}
            className="flex items-center gap-2 px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors"
          >
            <Plus className="w-4 h-4" />
            Create Identity
          </button>
        </div>
      </div>

      <div className="bg-white rounded-lg shadow mb-6 p-4">
        <div className="max-w-md">
          <label
            htmlFor="identity-search"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            <div className="flex items-center gap-2">
              <Search className="w-4 h-4" />
              Search Identities
            </div>
          </label>
          <input
            id="identity-search"
            type="text"
            value={searchTerm}
            onChange={(e) => {
              setSearchTerm(e.target.value);
              setPage(1);
            }}
            placeholder="Search by login, display name, or role..."
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        {filteredIdentities.length > 0 && (
          <div className="mt-3 text-sm text-gray-600">
            Showing {filteredIdentities.length} of {total} identities
            {searchTerm && " (filtered)"}
          </div>
        )}
      </div>

      <div className="bg-white rounded-lg shadow overflow-hidden">
        {isLoading ? (
          <div className="p-12 text-center">
            <div className="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
            <p className="mt-4 text-gray-600">Loading identities...</p>
          </div>
        ) : error ? (
          <div className="p-12 text-center">
            <p className="text-red-600">Failed to load identities</p>
            <p className="text-sm text-gray-600 mt-2">
              {error instanceof Error ? error.message : "Unknown error"}
            </p>
          </div>
        ) : !filteredIdentities || filteredIdentities.length === 0 ? (
          <div className="p-12 text-center">
            <Users className="mx-auto h-12 w-12 text-gray-400" />
            <p className="mt-4 text-gray-600">No identities found</p>
            <p className="text-sm text-gray-500 mt-1">
              {searchTerm
                ? "Try adjusting your search"
                : "Create your first identity to get started"}
            </p>
            {!searchTerm && (
              <button
                onClick={() => setShowCreateModal(true)}
                className="mt-4 px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors"
              >
                Create Identity
              </button>
            )}
          </div>
        ) : (
          <>
            <div className="overflow-x-auto">
              <table className="min-w-full divide-y divide-gray-200">
                <thead className="bg-gray-50">
                  <tr>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Login
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Display Name
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Roles
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Status
                    </th>
                    <th className="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody className="bg-white divide-y divide-gray-200">
                  {filteredIdentities.map((identity) => (
                    <tr key={identity.id} className="hover:bg-gray-50">
                      <td className="px-6 py-4 whitespace-nowrap">
                        <Link
                          to={"/access-control/identities/" + identity.id}
                          className="flex items-center gap-2 group"
                        >
                          <User className="w-4 h-4 text-gray-400 group-hover:text-blue-500" />
                          <span className="text-sm font-medium text-blue-600 group-hover:text-blue-800 group-hover:underline">
                            {identity.login}
                          </span>
                        </Link>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        <span className="text-sm text-gray-600">
                          {identity.display_name || "—"}
                        </span>
                      </td>
                      <td className="px-6 py-4">
                        <RoleBadges roles={identity.roles || []} />
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        {identity.frozen ? (
                          <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-semibold bg-blue-100 text-blue-800">
                            <Snowflake className="w-3 h-3" />
                            Frozen
                          </span>
                        ) : (
                          <span className="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-semibold bg-green-100 text-green-800">
                            Active
                          </span>
                        )}
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap text-right">
                        <button
                          onClick={() => handleToggleFreeze(identity)}
                          disabled={
                            freezeIdentity.isPending ||
                            unfreezeIdentity.isPending
                          }
                          className={
                            identity.frozen
                              ? "inline-flex items-center gap-1 px-2.5 py-1 text-xs font-medium rounded-md text-green-700 bg-green-50 hover:bg-green-100 border border-green-200 disabled:opacity-50 transition-colors"
                              : "inline-flex items-center gap-1 px-2.5 py-1 text-xs font-medium rounded-md text-blue-700 bg-blue-50 hover:bg-blue-100 border border-blue-200 disabled:opacity-50 transition-colors"
                          }
                          title={
                            identity.frozen
                              ? "Unfreeze identity"
                              : "Freeze identity"
                          }
                        >
                          {identity.frozen ? (
                            <>
                              <Sun className="w-3 h-3" />
                              Unfreeze
                            </>
                          ) : (
                            <>
                              <Snowflake className="w-3 h-3" />
                              Freeze
                            </>
                          )}
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </div>

      <Pagination
        page={page}
        setPage={setPage}
        pageSize={pageSize}
        itemCount={filteredIdentities.length}
        total={total}
        itemLabel="identities"
        floating
      />

      {showCreateModal && (
        <CreateIdentityModal onClose={() => setShowCreateModal(false)} />
      )}
    </div>
  );
}

function PermissionSetsTab() {
  const [searchTerm, setSearchTerm] = useState("");

  const { data: rawData, isLoading, error } = usePermissionSets();

  const permissionSets: PermissionSetRow[] =
    (rawData as PermissionSetRow[]) || [];

  const filteredSets = searchTerm
    ? permissionSets.filter(
        (ps) =>
          ps.ref.toLowerCase().includes(searchTerm.toLowerCase()) ||
          (ps.label || "").toLowerCase().includes(searchTerm.toLowerCase()) ||
          (ps.pack_ref || "").toLowerCase().includes(searchTerm.toLowerCase()),
      )
    : permissionSets;

  const grantsCount = (grants: unknown): number => {
    if (Array.isArray(grants)) return grants.length;
    if (typeof grants === "object" && grants !== null)
      return Object.keys(grants).length;
    return 0;
  };

  return (
    <>
      <div className="mb-6">
        <h2 className="text-xl font-semibold text-gray-900 flex items-center gap-2">
          <ShieldCheck className="w-5 h-5 text-indigo-600" />
          Permission Sets
        </h2>
        <p className="mt-1 text-sm text-gray-600">
          Browse permission sets and manage their role assignments
        </p>
      </div>

      <div className="bg-white rounded-lg shadow mb-6 p-4">
        <div className="max-w-md">
          <label
            htmlFor="permset-search"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            <div className="flex items-center gap-2">
              <Search className="w-4 h-4" />
              Search Permission Sets
            </div>
          </label>
          <input
            id="permset-search"
            type="text"
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            placeholder="Search by ref, label, or pack..."
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        {filteredSets.length > 0 && (
          <div className="mt-3 text-sm text-gray-600">
            Showing {filteredSets.length} of {permissionSets.length} permission
            sets{searchTerm && " (filtered)"}
          </div>
        )}
      </div>

      <div className="bg-white rounded-lg shadow overflow-hidden">
        {isLoading ? (
          <div className="p-12 text-center">
            <div className="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
            <p className="mt-4 text-gray-600">Loading permission sets...</p>
          </div>
        ) : error ? (
          <div className="p-12 text-center">
            <p className="text-red-600">Failed to load permission sets</p>
            <p className="text-sm text-gray-600 mt-2">
              {error instanceof Error ? error.message : "Unknown error"}
            </p>
          </div>
        ) : !filteredSets || filteredSets.length === 0 ? (
          <div className="p-12 text-center">
            <ShieldCheck className="mx-auto h-12 w-12 text-gray-400" />
            <p className="mt-4 text-gray-600">No permission sets found</p>
            <p className="text-sm text-gray-500 mt-1">
              {searchTerm
                ? "Try adjusting your search"
                : "Permission sets are defined in packs"}
            </p>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-200">
              <thead className="bg-gray-50">
                <tr>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Reference
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Label
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Pack
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Roles
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Grants
                  </th>
                </tr>
              </thead>
              <tbody className="bg-white divide-y divide-gray-200">
                {filteredSets.map((ps) => (
                  <tr key={ps.id} className="hover:bg-gray-50">
                    <td className="px-6 py-4 whitespace-nowrap">
                      <Link
                        to={"/access-control/permission-sets/" + ps.ref}
                        className="flex items-center gap-2 group"
                      >
                        <Shield className="w-4 h-4 text-indigo-400 group-hover:text-indigo-600" />
                        <span className="text-sm font-mono font-medium text-blue-600 group-hover:text-blue-800 group-hover:underline">
                          {ps.ref}
                        </span>
                      </Link>
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap">
                      <span className="text-sm text-gray-600">
                        {ps.label || "\u2014"}
                      </span>
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap">
                      {ps.pack_ref ? (
                        <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800">
                          <Package className="w-3 h-3" />
                          {ps.pack_ref}
                        </span>
                      ) : (
                        <span className="text-sm text-gray-400">
                          {"\u2014"}
                        </span>
                      )}
                    </td>
                    <td className="px-6 py-4">
                      <div className="flex flex-wrap gap-1">
                        {ps.roles && ps.roles.length > 0 ? (
                          ps.roles.map((ra) => (
                            <span
                              key={ra.id}
                              className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-purple-100 text-purple-800"
                            >
                              <Tag className="w-3 h-3" />
                              {ra.role}
                            </span>
                          ))
                        ) : (
                          <span className="text-xs text-gray-400">None</span>
                        )}
                      </div>
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap">
                      <span className="text-sm text-gray-600">
                        {grantsCount(ps.grants)} grant
                        {grantsCount(ps.grants) !== 1 ? "s" : ""}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </>
  );
}

export default function AccessControlPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const activeTab = searchParams.get("tab") || "identities";

  const setTab = (tab: string) => {
    setSearchParams({ tab });
  };

  return (
    <div className="p-6">
      <div className="mb-6">
        <h1 className="text-3xl font-bold text-gray-900 flex items-center gap-3">
          <Shield className="w-8 h-8 text-indigo-600" />
          Access Control
        </h1>
        <p className="mt-2 text-gray-600">
          Manage identities, permission sets, and role-based access control
        </p>
      </div>

      {/* Tabs */}
      <div className="border-b border-gray-200 mb-6">
        <nav className="-mb-px flex space-x-8">
          <button
            onClick={() => setTab("identities")}
            className={`whitespace-nowrap py-3 px-1 border-b-2 font-medium text-sm transition-colors ${
              activeTab === "identities"
                ? "border-blue-500 text-blue-600"
                : "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
            }`}
          >
            <div className="flex items-center gap-2">
              <Users className="w-4 h-4" />
              Identities
            </div>
          </button>
          <button
            onClick={() => setTab("permission-sets")}
            className={`whitespace-nowrap py-3 px-1 border-b-2 font-medium text-sm transition-colors ${
              activeTab === "permission-sets"
                ? "border-indigo-500 text-indigo-600"
                : "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
            }`}
          >
            <div className="flex items-center gap-2">
              <ShieldCheck className="w-4 h-4" />
              Permission Sets
            </div>
          </button>
        </nav>
      </div>

      {activeTab === "identities" ? <IdentitiesTab /> : <PermissionSetsTab />}
    </div>
  );
}
