import { Link, useParams, useNavigate } from "react-router-dom";
import {
  useTriggers,
  useTrigger,
  useDeleteTrigger,
  useEnableTrigger,
  useDisableTrigger,
} from "@/hooks/useTriggers";
import { useState, useMemo } from "react";
import type { TriggerSummary } from "@/api";
import {
  extractProperties,
  type ParamSchemaProperty,
} from "@/components/common/ParamSchemaForm";
import OnOffSwitch from "@/components/common/OnOffSwitch";
import {
  ChevronDown,
  ChevronRight,
  Search,
  X,
  Plus,
  Copy,
  Check,
  Pencil,
} from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";
import PackIcon from "@/components/common/PackIcon";

export default function TriggersPage() {
  const { ref } = useParams<{ ref?: string }>();
  const { data, isLoading, error } = useTriggers({});
  const triggers = useMemo(() => data?.items || [], [data?.items]);
  const [collapsedPacks, setCollapsedPacks] = useState<Set<string>>(new Set());
  const [searchQuery, setSearchQuery] = useState("");

  // Filter triggers based on search query
  const filteredTriggers = useMemo(() => {
    if (!searchQuery.trim()) return triggers;
    const query = searchQuery.toLowerCase();
    return triggers.filter((trigger: TriggerSummary) => {
      return (
        trigger.label?.toLowerCase().includes(query) ||
        trigger.ref?.toLowerCase().includes(query) ||
        trigger.description?.toLowerCase().includes(query) ||
        trigger.pack_ref?.toLowerCase().includes(query)
      );
    });
  }, [triggers, searchQuery]);

  // Group filtered triggers by pack
  const triggersByPack = useMemo(() => {
    const grouped = new Map<string, TriggerSummary[]>();
    filteredTriggers.forEach((trigger: TriggerSummary) => {
      const packRef = trigger.pack_ref || "unknown";
      if (!grouped.has(packRef)) {
        grouped.set(packRef, []);
      }
      grouped.get(packRef)!.push(trigger);
    });
    // Sort packs alphabetically
    return new Map(
      [...grouped.entries()].sort((a, b) => a[0].localeCompare(b[0])),
    );
  }, [filteredTriggers]);

  const togglePack = (packRef: string) => {
    setCollapsedPacks((prev) => {
      const next = new Set(prev);
      if (next.has(packRef)) {
        next.delete(packRef);
      } else {
        next.add(packRef);
      }
      return next;
    });
  };

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {(error as Error).message}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full">
      {/* Left sidebar - Triggers List */}
      <div className="w-96 border-r border-gray-200 overflow-y-auto bg-gray-50">
        <div className="p-4 border-b border-gray-200 bg-white sticky top-0 z-10">
          <div className="flex items-center justify-between mb-1">
            <h1 className="text-2xl font-bold">Triggers</h1>
            <Link
              to="/triggers/create"
              className="inline-flex items-center px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700"
            >
              <Plus className="h-4 w-4 mr-1" />
              Create Trigger
            </Link>
          </div>
          <p className="text-sm text-gray-600 mt-1">
            {filteredTriggers.length} of {triggers.length} triggers
          </p>

          {/* Search Bar */}
          <div className="mt-3 relative">
            <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
              <Search className="h-4 w-4 text-gray-400" />
            </div>
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search triggers..."
              className="block w-full pl-10 pr-10 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery("")}
                className="absolute inset-y-0 right-0 pr-3 flex items-center"
              >
                <X className="h-4 w-4 text-gray-400 hover:text-gray-600" />
              </button>
            )}
          </div>
        </div>
        <div className="p-2">
          {triggers.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No triggers found</p>
            </div>
          ) : filteredTriggers.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No triggers match your search</p>
              <button
                onClick={() => setSearchQuery("")}
                className="mt-2 text-sm text-blue-600 hover:text-blue-800"
              >
                Clear search
              </button>
            </div>
          ) : (
            <div className="space-y-2">
              {Array.from(triggersByPack.entries()).map(
                ([packRef, packTriggers]) => {
                  const isCollapsed = collapsedPacks.has(packRef);
                  return (
                    <div
                      key={packRef}
                      className="bg-white rounded-lg shadow-sm overflow-hidden"
                    >
                      {/* Pack Header */}
                      <button
                        onClick={() => togglePack(packRef)}
                        className="w-full px-3 py-2 flex items-center justify-between hover:bg-gray-50 transition-colors border-b border-gray-200"
                      >
                        <div className="flex items-center gap-2">
                          {isCollapsed ? (
                            <ChevronRight className="w-4 h-4 text-gray-500" />
                          ) : (
                            <ChevronDown className="w-4 h-4 text-gray-500" />
                          )}
                          <PackIcon packRef={packRef} size="xs" />
                          <span className="font-semibold text-sm text-gray-900">
                            {packRef}
                          </span>
                        </div>
                        <span className="text-xs text-gray-500 bg-gray-100 px-2 py-0.5 rounded">
                          {packTriggers.length}
                        </span>
                      </button>

                      {/* Triggers List */}
                      {!isCollapsed && (
                        <div className="p-1">
                          {packTriggers.map((trigger: TriggerSummary) => (
                            <Link
                              key={trigger.id}
                              to={`/triggers/${trigger.ref}`}
                              className={`block p-3 rounded transition-colors ${
                                ref === trigger.ref
                                  ? "bg-blue-50 border-2 border-blue-500"
                                  : "border-2 border-transparent hover:bg-gray-50"
                              }`}
                            >
                              <div className="flex items-center justify-between">
                                <div className="min-w-0 flex items-center gap-2">
                                  <PackIcon
                                    packRef={trigger.pack_ref}
                                    size="sm"
                                  />
                                  <div className="font-medium text-sm text-gray-900 truncate">
                                    {trigger.label}
                                  </div>
                                </div>
                                <span
                                  className={`ml-2 inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium ${
                                    trigger.enabled
                                      ? "bg-green-100 text-green-800"
                                      : "bg-gray-100 text-gray-800"
                                  }`}
                                >
                                  {trigger.enabled ? "Enabled" : "Disabled"}
                                </span>
                              </div>
                              <div className="font-mono text-xs text-gray-500 mt-1 truncate">
                                {trigger.ref}
                              </div>
                              <div className="mt-2">
                                <span className="text-xs text-gray-500 bg-blue-50 px-2 py-0.5 rounded">
                                  {trigger.reference_visibility}
                                </span>
                              </div>
                              {trigger.description && (
                                <div className="text-xs text-gray-400 mt-1 line-clamp-2">
                                  {trigger.description}
                                </div>
                              )}
                            </Link>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                },
              )}
            </div>
          )}
        </div>
      </div>

      {/* Right panel - Trigger Detail or Empty State */}
      <div className="flex-1 overflow-y-auto">
        {ref ? (
          <TriggerDetail triggerRef={ref} />
        ) : (
          <div className="flex items-center justify-center h-full">
            <div className="text-center text-gray-500">
              <svg
                className="mx-auto h-12 w-12 text-gray-400"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth={2}
                  d="M13 10V3L4 14h7v7l9-11h-7z"
                />
              </svg>
              <h3 className="mt-2 text-sm font-medium text-gray-900">
                No trigger selected
              </h3>
              <p className="mt-1 text-sm text-gray-500">
                Select a trigger from the list to view its details
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function TriggerDetail({ triggerRef }: { triggerRef: string }) {
  const navigate = useNavigate();
  const { data: trigger, isLoading, error } = useTrigger(triggerRef);
  const { isAuthenticated } = useAuth();
  const deleteTrigger = useDeleteTrigger();
  const enableTrigger = useEnableTrigger();
  const disableTrigger = useDisableTrigger();
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [isTogglingEnabled, setIsTogglingEnabled] = useState(false);
  const [copiedWebhookUrl, setCopiedWebhookUrl] = useState(false);

  const handleToggleEnabled = async () => {
    if (!trigger?.data) return;

    setIsTogglingEnabled(true);
    try {
      if (trigger.data.enabled) {
        await disableTrigger.mutateAsync(triggerRef);
      } else {
        await enableTrigger.mutateAsync(triggerRef);
      }
    } catch (err) {
      console.error("Failed to toggle trigger enabled status:", err);
    } finally {
      setIsTogglingEnabled(false);
    }
  };

  const handleDelete = async () => {
    try {
      await deleteTrigger.mutateAsync(triggerRef);
      window.location.href = "/triggers";
    } catch (err) {
      console.error("Failed to delete trigger:", err);
    }
  };

  const copyWebhookUrl = async () => {
    if (!trigger?.data?.webhook_key) return;

    const apiBaseUrl =
      import.meta.env.VITE_API_BASE_URL || window.location.origin;
    const webhookUrl = `${apiBaseUrl}/webhooks/${trigger.data.webhook_key}`;

    try {
      await navigator.clipboard.writeText(webhookUrl);
      setCopiedWebhookUrl(true);
      setTimeout(() => setCopiedWebhookUrl(false), 2000);
    } catch (err) {
      console.error("Failed to copy webhook URL:", err);
    }
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  if (error || !trigger) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {error ? (error as Error).message : "Trigger not found"}</p>
        </div>
      </div>
    );
  }

  const paramSchema = trigger.data?.param_schema || {};
  const properties = extractProperties(paramSchema);
  const paramEntries = Object.entries(properties);

  const outSchema = trigger.data?.out_schema || {};
  const outProperties = extractProperties(outSchema);
  const outEntries = Object.entries(outProperties);

  return (
    <div className="p-6 max-w-7xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <h1 className="text-3xl font-bold">
              <span className="text-gray-500">{trigger.data?.pack_ref}.</span>
              {trigger.data?.label}
            </h1>
            <div className="inline-flex items-center">
              <OnOffSwitch
                checked={trigger.data?.enabled || false}
                disabled={!isAuthenticated || isTogglingEnabled}
                ariaLabel="Trigger enabled"
                onChange={() => {
                  handleToggleEnabled();
                }}
              />
              <span className="ms-3 text-sm font-medium text-gray-900">
                {isTogglingEnabled ? (
                  <span className="text-gray-400">Updating...</span>
                ) : (
                  <span
                    className={
                      trigger.data?.enabled ? "text-green-700" : "text-gray-700"
                    }
                  >
                    {trigger.data?.enabled ? "Enabled" : "Disabled"}
                  </span>
                )}
              </span>
            </div>
          </div>
          <div className="flex gap-2">
            {/* Show edit and delete buttons for ad-hoc triggers (not from pack installation) */}
            {trigger.data?.is_adhoc && (
              <>
                <button
                  onClick={() =>
                    navigate(`/triggers/${encodeURIComponent(triggerRef)}/edit`)
                  }
                  className="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700 inline-flex items-center gap-2"
                >
                  <Pencil className="h-4 w-4" />
                  Edit
                </button>
                <button
                  onClick={() => setShowDeleteConfirm(true)}
                  disabled={deleteTrigger.isPending}
                  className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700 disabled:opacity-50"
                >
                  Delete
                </button>
              </>
            )}
          </div>
        </div>
      </div>

      {/* Delete Confirmation Modal */}
      {showDeleteConfirm && (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
          <div className="bg-white rounded-lg p-6 max-w-md">
            <h3 className="text-xl font-bold mb-4">Confirm Delete</h3>
            <p className="mb-6">
              Are you sure you want to delete trigger{" "}
              <strong>
                {trigger.data?.pack_ref}.{trigger.data?.label}
              </strong>
              ?
            </p>
            <div className="flex justify-end gap-3">
              <button
                onClick={() => setShowDeleteConfirm(false)}
                className="px-4 py-2 bg-gray-200 rounded hover:bg-gray-300"
              >
                Cancel
              </button>
              <button
                onClick={handleDelete}
                className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Main Info Card */}
        <div className="lg:col-span-2 space-y-6">
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-xl font-semibold mb-4">Trigger Information</h2>
            <dl className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div>
                <dt className="text-sm font-medium text-gray-500">Reference</dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {trigger.data?.ref}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Label</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {trigger.data?.label}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Pack</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  <Link
                    to={`/packs/${trigger.data?.pack_ref}`}
                    className="text-blue-600 hover:text-blue-800"
                  >
                    {trigger.data?.pack_ref}
                  </Link>
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">
                  Rule subscription visibility
                </dt>
                <dd className="mt-1 text-sm text-gray-900 capitalize">
                  {trigger.data?.reference_visibility}
                </dd>
              </div>
              {trigger.data?.reference_visibility === "restricted" && (
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Allowed caller packs
                  </dt>
                  <dd className="mt-1 text-sm text-gray-900">
                    {trigger.data.reference_allowed_pack_refs?.length
                      ? trigger.data.reference_allowed_pack_refs.join(", ")
                      : "Only this pack"}
                  </dd>
                </div>
              )}
              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-gray-500">
                  Description
                </dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {trigger.data?.description || "No description provided"}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Created</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(trigger.data?.created || "").toLocaleString()}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Updated</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(trigger.data?.updated || "").toLocaleString()}
                </dd>
              </div>
            </dl>

            {paramEntries.length > 0 && (
              <div className="mt-6">
                <h3 className="text-sm font-medium text-gray-900 mb-3">
                  Parameters
                </h3>
                <div className="space-y-3">
                  {paramEntries.map(
                    ([key, param]: [string, ParamSchemaProperty]) => (
                      <div
                        key={key}
                        className="border border-gray-200 rounded p-3"
                      >
                        <div className="flex items-start justify-between">
                          <div className="flex-1">
                            <div className="flex items-center gap-2">
                              <span className="font-mono font-semibold text-sm">
                                {key}
                              </span>
                              {param?.required && (
                                <span className="text-xs px-2 py-0.5 bg-red-100 text-red-700 rounded">
                                  Required
                                </span>
                              )}
                              {param?.secret && (
                                <span className="text-xs px-2 py-0.5 bg-yellow-100 text-yellow-700 rounded">
                                  Secret
                                </span>
                              )}
                              <span className="text-xs px-2 py-0.5 bg-gray-100 text-gray-700 rounded">
                                {param?.type || "any"}
                              </span>
                            </div>
                            {param?.description && (
                              <p className="text-sm text-gray-600 mt-1">
                                {param.description}
                              </p>
                            )}
                            {param?.default !== undefined && (
                              <p className="text-xs text-gray-500 mt-1">
                                Default:{" "}
                                <code className="bg-gray-100 px-1 rounded">
                                  {JSON.stringify(param.default)}
                                </code>
                              </p>
                            )}
                          </div>
                        </div>
                      </div>
                    ),
                  )}
                </div>
              </div>
            )}
          </div>

          {/* Payload Schema Card */}
          {outEntries.length > 0 && (
            <div className="bg-white shadow rounded-lg p-6">
              <h2 className="text-xl font-semibold mb-2">Payload Schema</h2>
              <p className="text-sm text-gray-500 mb-4">
                Schema of the event payload generated when this trigger fires.
              </p>
              <div className="space-y-3">
                {outEntries.map(
                  ([key, param]: [string, ParamSchemaProperty]) => (
                    <div
                      key={key}
                      className="border border-gray-200 rounded p-3"
                    >
                      <div className="flex items-start justify-between">
                        <div className="flex-1">
                          <div className="flex items-center gap-2">
                            <span className="font-mono font-semibold text-sm">
                              {key}
                            </span>
                            {param?.required && (
                              <span className="text-xs px-2 py-0.5 bg-red-100 text-red-700 rounded">
                                Required
                              </span>
                            )}
                            <span className="text-xs px-2 py-0.5 bg-gray-100 text-gray-700 rounded">
                              {param?.type || "any"}
                            </span>
                          </div>
                          {param?.description && (
                            <p className="text-sm text-gray-600 mt-1">
                              {param.description}
                            </p>
                          )}
                          {param?.default !== undefined && (
                            <p className="text-xs text-gray-500 mt-1">
                              Default:{" "}
                              <code className="bg-gray-100 px-1 rounded">
                                {JSON.stringify(param.default)}
                              </code>
                            </p>
                          )}
                        </div>
                      </div>
                    </div>
                  ),
                )}
              </div>
            </div>
          )}
        </div>

        {/* Sidebar */}
        <div className="space-y-6">
          {/* Webhook URL (if enabled) */}
          {trigger.data?.webhook_enabled && trigger.data?.webhook_key && (
            <div className="bg-white shadow rounded-lg p-6">
              <h2 className="text-lg font-semibold mb-4">Webhook URL</h2>
              <div className="space-y-3">
                <div className="p-3 bg-gray-50 rounded border border-gray-200">
                  <code className="text-xs break-all text-gray-700">
                    {import.meta.env.VITE_API_BASE_URL ||
                      window.location.origin}
                    /webhooks/{trigger.data.webhook_key}
                  </code>
                </div>
                <button
                  onClick={copyWebhookUrl}
                  className="w-full flex items-center justify-center gap-2 px-4 py-2 text-sm bg-blue-600 text-white rounded hover:bg-blue-700"
                >
                  {copiedWebhookUrl ? (
                    <>
                      <Check className="h-4 w-4" />
                      Copied!
                    </>
                  ) : (
                    <>
                      <Copy className="h-4 w-4" />
                      Copy URL
                    </>
                  )}
                </button>
                <p className="text-xs text-gray-500">
                  Use this URL to send webhook events to this trigger.
                </p>
              </div>
            </div>
          )}

          {/* Quick Actions */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Quick Actions</h2>
            <div className="space-y-2">
              <Link
                to={`/packs/${trigger.data?.pack_ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Pack
              </Link>
              <Link
                to={`/rules?trigger=${trigger.data?.ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Rules
              </Link>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
