import { useState } from "react";
import { useKeys, useDeleteKey } from "@/hooks/useKeys";
import { OwnerType } from "@/api";
import {
  Check,
  Copy,
  Key,
  Plus,
  Trash2,
  Edit,
  Eye,
  EyeOff,
  Search,
} from "lucide-react";
import Pagination from "@/components/executions/Pagination";
import KeyCreateModal from "./KeyCreateModal";
import KeyEditModal from "./KeyEditModal";
import KeyOwnerDisplay from "./KeyOwnerDisplay";

export default function KeysPage() {
  const [page, setPage] = useState(1);
  const [searchTerm, setSearchTerm] = useState("");
  const [ownerTypeFilter, setOwnerTypeFilter] = useState<OwnerType | "">("");
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [copiedReference, setCopiedReference] = useState<string | null>(null);
  const pageSize = 20;

  const { data, isLoading, error } = useKeys({
    page,
    pageSize,
    ownerType: ownerTypeFilter || undefined,
  });

  const deleteKeyMutation = useDeleteKey();

  const keys = data?.items || [];
  const total = data?.pagination?.total_items || 0;

  // Client-side filtering by search term (ref or name)
  const filteredKeys = searchTerm
    ? keys.filter(
        (key) =>
          key.ref.toLowerCase().includes(searchTerm.toLowerCase()) ||
          key.name.toLowerCase().includes(searchTerm.toLowerCase()),
      )
    : keys;

  const hasActiveFilters = searchTerm || ownerTypeFilter;

  const clearFilters = () => {
    setSearchTerm("");
    setOwnerTypeFilter("");
    setPage(1);
  };

  const handleDelete = async (ref: string) => {
    if (window.confirm(`Are you sure you want to delete key "${ref}"?`)) {
      try {
        await deleteKeyMutation.mutateAsync(ref);
      } catch (err) {
        console.error("Failed to delete key:", err);
        alert("Failed to delete key. Please try again.");
      }
    }
  };

  const copyReference = async (ref: string) => {
    try {
      await navigator.clipboard.writeText(ref);
      setCopiedReference(ref);
      window.setTimeout(() => setCopiedReference(null), 1500);
    } catch {
      setCopiedReference(null);
    }
  };

  const formatDate = (dateString: string) => {
    return new Date(dateString).toLocaleString();
  };

  return (
    <div className="p-6 pb-28">
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-3xl font-bold text-gray-900">Keys & Secrets</h1>
            <p className="mt-2 text-gray-600">
              Manage encrypted secrets and configuration values
            </p>
          </div>
          <button
            onClick={() => setShowCreateModal(true)}
            className="flex items-center gap-2 px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors"
          >
            <Plus className="w-4 h-4" />
            Create Key
          </button>
        </div>
      </div>

      <div className="bg-white rounded-lg shadow mb-6 p-4">
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
          <div>
            <label
              htmlFor="search"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              <div className="flex items-center gap-2">
                <Search className="w-4 h-4" />
                Search Keys
              </div>
            </label>
            <input
              id="search"
              type="text"
              value={searchTerm}
              onChange={(e) => {
                setSearchTerm(e.target.value);
                setPage(1);
              }}
              placeholder="Search by reference or name..."
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>

          <div>
            <label
              htmlFor="owner-type-filter"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Filter by Scope
            </label>
            <select
              id="owner-type-filter"
              value={ownerTypeFilter}
              onChange={(e) => {
                setOwnerTypeFilter(e.target.value as OwnerType | "");
                setPage(1);
              }}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              <option value="">All Scopes</option>
              <option value={OwnerType.SYSTEM}>System</option>
              <option value={OwnerType.IDENTITY}>User</option>
              <option value={OwnerType.PACK}>Pack</option>
              <option value={OwnerType.ACTION}>Action</option>
              <option value={OwnerType.SENSOR}>Sensor</option>
            </select>
          </div>
        </div>

        <div className="flex items-center justify-between">
          <div className="text-sm text-gray-600">
            {filteredKeys.length > 0 && (
              <>
                Showing {filteredKeys.length} of {total} keys
                {hasActiveFilters && " (filtered)"}
              </>
            )}
          </div>
          {hasActiveFilters && (
            <button
              onClick={clearFilters}
              className="px-4 py-2 text-sm text-gray-600 hover:text-gray-900"
            >
              Clear Filters
            </button>
          )}
        </div>
      </div>

      <div className="bg-white rounded-lg shadow overflow-hidden">
        {isLoading ? (
          <div className="p-12 text-center">
            <div className="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
            <p className="mt-4 text-gray-600">Loading keys...</p>
          </div>
        ) : error ? (
          <div className="p-12 text-center">
            <p className="text-red-600">Failed to load keys</p>
            <p className="text-sm text-gray-600 mt-2">
              {error instanceof Error ? error.message : "Unknown error"}
            </p>
          </div>
        ) : !filteredKeys || filteredKeys.length === 0 ? (
          <div className="p-12 text-center">
            <Key className="mx-auto h-12 w-12 text-gray-400" />
            <p className="mt-4 text-gray-600">No keys found</p>
            <p className="text-sm text-gray-500 mt-1">
              {hasActiveFilters
                ? "Try adjusting your filters"
                : "Create your first key to get started"}
            </p>
            {!hasActiveFilters && (
              <button
                onClick={() => setShowCreateModal(true)}
                className="mt-4 px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors"
              >
                Create Key
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
                      Reference
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Name
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Owner
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Encrypted
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Created
                    </th>
                    <th className="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody className="bg-white divide-y divide-gray-200">
                  {filteredKeys.map((key) => (
                    <tr key={key.id} className="hover:bg-gray-50">
                      <td className="px-6 py-4 whitespace-nowrap">
                        <div className="flex items-center gap-2">
                          <Key className="w-4 h-4 text-gray-400" />
                          <span className="text-sm font-mono text-gray-900">
                            {key.ref}
                          </span>
                          <button
                            type="button"
                            onClick={() => void copyReference(key.ref)}
                            className="rounded p-1 text-gray-400 hover:bg-gray-100 hover:text-blue-600"
                            aria-label={
                              copiedReference === key.ref
                                ? `Reference copied: ${key.ref}`
                                : `Copy reference: ${key.ref}`
                            }
                            title={
                              copiedReference === key.ref
                                ? "Reference copied"
                                : "Copy reference"
                            }
                          >
                            {copiedReference === key.ref ? (
                              <Check className="h-4 w-4 text-green-600" />
                            ) : (
                              <Copy className="h-4 w-4" />
                            )}
                          </button>
                        </div>
                      </td>
                      <td className="px-6 py-4">
                        <div className="text-sm text-gray-900">{key.name}</div>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        <KeyOwnerDisplay
                          ownerType={key.owner_type}
                          ownerRef={key.owner}
                        />
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        <div className="flex items-center gap-2">
                          {key.encrypted ? (
                            <>
                              <EyeOff className="w-4 h-4 text-green-600" />
                              <span className="text-sm text-green-600 font-medium">
                                Yes
                              </span>
                            </>
                          ) : (
                            <>
                              <Eye className="w-4 h-4 text-gray-400" />
                              <span className="text-sm text-gray-600">No</span>
                            </>
                          )}
                        </div>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        <div className="text-sm text-gray-900">
                          {formatDate(key.created)}
                        </div>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                        <div className="flex items-center justify-end gap-2">
                          <button
                            onClick={() => setEditingKey(key.ref)}
                            className="text-blue-600 hover:text-blue-900"
                            title="Edit key"
                          >
                            <Edit className="w-4 h-4" />
                          </button>
                          <button
                            onClick={() => handleDelete(key.ref)}
                            className="text-red-600 hover:text-red-900"
                            title="Delete key"
                          >
                            <Trash2 className="w-4 h-4" />
                          </button>
                        </div>
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
        itemCount={filteredKeys.length}
        total={total}
        itemLabel="keys"
        floating
      />

      {showCreateModal && (
        <KeyCreateModal onClose={() => setShowCreateModal(false)} />
      )}
      {editingKey && (
        <KeyEditModal keyRef={editingKey} onClose={() => setEditingKey(null)} />
      )}
    </div>
  );
}
