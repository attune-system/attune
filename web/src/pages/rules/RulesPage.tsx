import { Link, useParams } from "react-router-dom";
import {
  useRules,
  useRule,
  useDeleteRule,
  useEnableRule,
  useDisableRule,
} from "@/hooks/useRules";
import { useTrigger } from "@/hooks/useTriggers";
import { useAction } from "@/hooks/useActions";
import { useState, useMemo } from "react";
import type { RuleSummary } from "@/api";
import { ChevronDown, ChevronRight, Search, X } from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";
import OnOffSwitch from "@/components/common/OnOffSwitch";
import ParamSchemaDisplay, {
  type ParamSchema,
} from "@/components/common/ParamSchemaDisplay";
import PackIcon from "@/components/common/PackIcon";

export default function RulesPage() {
  const { ref } = useParams<{ ref?: string }>();
  const { data, isLoading, error } = useRules({});
  const rules = useMemo(() => data?.items || [], [data?.items]);
  const [collapsedPacks, setCollapsedPacks] = useState<Set<string>>(new Set());
  const [searchQuery, setSearchQuery] = useState("");

  // Filter rules based on search query
  const filteredRules = useMemo(() => {
    if (!searchQuery.trim()) return rules;
    const query = searchQuery.toLowerCase();
    return rules.filter((rule: RuleSummary) => {
      return (
        rule.label?.toLowerCase().includes(query) ||
        rule.ref?.toLowerCase().includes(query) ||
        rule.description?.toLowerCase().includes(query) ||
        rule.pack_ref?.toLowerCase().includes(query) ||
        rule.trigger_ref?.toLowerCase().includes(query) ||
        rule.action_ref?.toLowerCase().includes(query)
      );
    });
  }, [rules, searchQuery]);

  // Group filtered rules by pack
  const rulesByPack = useMemo(() => {
    const grouped = new Map<string, RuleSummary[]>();
    filteredRules.forEach((rule: RuleSummary) => {
      const packRef = rule.pack_ref || "unknown";
      if (!grouped.has(packRef)) {
        grouped.set(packRef, []);
      }
      grouped.get(packRef)!.push(rule);
    });
    // Sort packs alphabetically
    return new Map(
      [...grouped.entries()].sort((a, b) => a[0].localeCompare(b[0])),
    );
  }, [filteredRules]);

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
      {/* Left sidebar - Rules List */}
      <div className="w-96 border-r border-gray-200 overflow-y-auto bg-gray-50">
        <div className="p-4 border-b border-gray-200 bg-white sticky top-0 z-10">
          <div className="flex items-center justify-between mb-2">
            <h1 className="text-2xl font-bold">Rules</h1>
            <Link
              to="/rules/new"
              className="px-3 py-1 bg-blue-600 text-white rounded hover:bg-blue-700 text-sm font-medium"
            >
              + New
            </Link>
          </div>
          <p className="text-sm text-gray-600">
            {filteredRules.length} of {rules.length} rules
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
              placeholder="Search rules..."
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
          {rules.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No rules found</p>
              <Link
                to="/rules/new"
                className="mt-3 inline-block text-sm text-blue-600 hover:text-blue-800"
              >
                Create your first rule
              </Link>
            </div>
          ) : filteredRules.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No rules match your search</p>
              <button
                onClick={() => setSearchQuery("")}
                className="mt-2 text-sm text-blue-600 hover:text-blue-800"
              >
                Clear search
              </button>
            </div>
          ) : (
            <div className="space-y-2">
              {Array.from(rulesByPack.entries()).map(([packRef, packRules]) => {
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
                        {packRules.length}
                      </span>
                    </button>

                    {/* Rules List */}
                    {!isCollapsed && (
                      <div className="p-1">
                        {packRules.map((rule: RuleSummary) => (
                          <Link
                            key={rule.id}
                            to={`/rules/${rule.ref}`}
                            className={`block p-3 rounded transition-colors ${
                              ref === rule.ref
                                ? "bg-blue-50 border-2 border-blue-500"
                                : "border-2 border-transparent hover:bg-gray-50"
                            }`}
                          >
                            <div className="flex items-center justify-between">
                              <div className="min-w-0 flex items-center gap-2">
                                <PackIcon packRef={rule.pack_ref} size="sm" />
                                <div className="font-medium text-sm text-gray-900 truncate">
                                  {rule.label}
                                </div>
                              </div>
                              <span
                                className={`ml-2 inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium ${
                                  rule.enabled
                                    ? "bg-green-100 text-green-800"
                                    : "bg-gray-100 text-gray-800"
                                }`}
                              >
                                {rule.enabled ? "Enabled" : "Disabled"}
                              </span>
                            </div>
                            <div className="font-mono text-xs text-gray-500 mt-1 truncate">
                              {rule.ref}
                            </div>
                            <div className="text-xs text-gray-400 mt-1 truncate">
                              {rule.trigger_ref} → {rule.action_ref}
                            </div>
                          </Link>
                        ))}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>

      {/* Right panel - Rule Detail or Empty State */}
      <div className="flex-1 overflow-y-auto">
        {ref ? (
          <RuleDetail ruleRef={ref} />
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
                  d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2"
                />
              </svg>
              <h3 className="mt-2 text-sm font-medium text-gray-900">
                No rule selected
              </h3>
              <p className="mt-1 text-sm text-gray-500">
                Select a rule from the list to view its details
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function RuleDetail({ ruleRef }: { ruleRef: string }) {
  const { data: rule, isLoading, error } = useRule(ruleRef);
  const { isAuthenticated } = useAuth();
  const deleteRule = useDeleteRule();
  const enableRule = useEnableRule();
  const disableRule = useDisableRule();
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [isTogglingEnabled, setIsTogglingEnabled] = useState(false);

  // Fetch trigger and action details to get param schemas
  const { data: triggerData } = useTrigger(rule?.data?.trigger_ref || "");
  const { data: actionData } = useAction(rule?.data?.action_ref || "");

  const triggerParamSchema: ParamSchema =
    (triggerData?.data as { param_schema?: ParamSchema })?.param_schema || {};
  const actionParamSchema: ParamSchema =
    (actionData?.data as { param_schema?: ParamSchema })?.param_schema || {};

  const handleToggleEnabled = async () => {
    if (!rule?.data) return;

    setIsTogglingEnabled(true);
    try {
      if (rule.data.enabled) {
        await disableRule.mutateAsync(ruleRef);
      } else {
        await enableRule.mutateAsync(ruleRef);
      }
    } catch (err) {
      console.error("Failed to toggle rule enabled status:", err);
    } finally {
      setIsTogglingEnabled(false);
    }
  };

  const handleDelete = async () => {
    try {
      await deleteRule.mutateAsync(ruleRef);
      window.location.href = "/rules";
    } catch (err) {
      console.error("Failed to delete rule:", err);
    }
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  if (error || !rule) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {error ? (error as Error).message : "Rule not found"}</p>
        </div>
      </div>
    );
  }

  const traceTagTemplate = (rule.data as typeof rule.data & {
    trace_tag_template?: string | null;
  }).trace_tag_template;

  return (
    <div className="p-6 max-w-7xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <h1 className="text-3xl font-bold">
              <span className="text-gray-500">{rule.data?.pack_ref}.</span>
              {rule.data?.label}
            </h1>
            <div className="inline-flex items-center">
              <OnOffSwitch
                checked={rule.data?.enabled || false}
                disabled={!isAuthenticated || isTogglingEnabled}
                ariaLabel="Rule enabled"
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
                      rule.data?.enabled ? "text-green-700" : "text-gray-700"
                    }
                  >
                    {rule.data?.enabled ? "Enabled" : "Disabled"}
                  </span>
                )}
              </span>
            </div>
          </div>
          <div className="flex gap-2">
            <Link
              to={`/rules/${ruleRef}/edit`}
              className="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700"
            >
              Edit
            </Link>
            <button
              onClick={() => setShowDeleteConfirm(true)}
              disabled={deleteRule.isPending}
              className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700 disabled:opacity-50"
            >
              Delete
            </button>
          </div>
        </div>
      </div>

      {/* Delete Confirmation Modal */}
      {showDeleteConfirm && (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
          <div className="bg-white rounded-lg p-6 max-w-md">
            <h3 className="text-xl font-bold mb-4">Confirm Delete</h3>
            <p className="mb-6">
              Are you sure you want to delete rule{" "}
              <strong>
                {rule.data?.pack_ref}.{rule.data?.label}
              </strong>
              ? This will prevent the rule from triggering any actions.
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
            <h2 className="text-xl font-semibold mb-4">Rule Information</h2>
            <dl className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div>
                <dt className="text-sm font-medium text-gray-500">Reference</dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {rule.data?.ref}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Label</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {rule.data?.label}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Pack</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  <Link
                    to={`/packs/${rule.data?.pack_ref}`}
                    className="text-blue-600 hover:text-blue-800"
                  >
                    {rule.data?.pack_ref}
                  </Link>
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Status</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {rule.data?.enabled ? "Enabled" : "Disabled"}
                </dd>
              </div>
              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-gray-500">
                  Description
                </dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {rule.data?.description || "No description provided"}
                </dd>
              </div>
              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-gray-500">
                  Trace tag template
                </dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {traceTagTemplate || "Default (<trigger_ref>.<event_id>)"}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Created</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(rule.data?.created || "").toLocaleString()}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Updated</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(rule.data?.updated || "").toLocaleString()}
                </dd>
              </div>
            </dl>
          </div>

          {/* Trigger and Action */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-xl font-semibold mb-4">Configuration</h2>
            <div className="space-y-4">
              <div className="flex items-center">
                <div className="flex-1">
                  <dt className="text-sm font-medium text-gray-500 mb-1">
                    Trigger
                  </dt>
                  <dd className="text-sm text-gray-900">
                    <Link
                      to={`/triggers/${rule.data?.trigger_ref}`}
                      className="text-blue-600 hover:text-blue-800 font-mono"
                    >
                      {rule.data?.trigger_ref}
                    </Link>
                  </dd>
                </div>
                <div className="px-4">
                  <svg
                    className="h-6 w-6 text-gray-400"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={2}
                      d="M13 7l5 5m0 0l-5 5m5-5H6"
                    />
                  </svg>
                </div>
                <div className="flex-1">
                  <dt className="text-sm font-medium text-gray-500 mb-1">
                    Action
                  </dt>
                  <dd className="text-sm text-gray-900">
                    <Link
                      to={`/actions/${rule.data?.action_ref}`}
                      className="text-blue-600 hover:text-blue-800 font-mono"
                    >
                      {rule.data?.action_ref}
                    </Link>
                  </dd>
                </div>
              </div>

              {rule.data?.conditions &&
                Object.keys(rule.data.conditions).length > 0 && (
                  <div className="mt-4">
                    <dt className="text-sm font-medium text-gray-500 mb-2">
                      Conditions
                    </dt>
                    <pre className="bg-gray-50 p-3 rounded text-xs overflow-x-auto">
                      {JSON.stringify(rule.data.conditions, null, 2)}
                    </pre>
                  </div>
                )}

              {/* Trigger Parameters - Human-friendly display */}
              {rule.data?.trigger_params &&
                Object.keys(rule.data.trigger_params).length > 0 && (
                  <div className="mt-4">
                    <dt className="text-sm font-medium text-gray-500 mb-3">
                      Trigger Parameters
                    </dt>
                    <ParamSchemaDisplay
                      schema={triggerParamSchema}
                      values={rule.data.trigger_params}
                      emptyMessage="No trigger parameters configured"
                    />
                  </div>
                )}

              {/* Action Parameters - Human-friendly display */}
              {rule.data?.action_params &&
                Object.keys(rule.data.action_params).length > 0 && (
                  <div className="mt-4">
                    <dt className="text-sm font-medium text-gray-500 mb-3">
                      Action Parameters
                    </dt>
                    <ParamSchemaDisplay
                      schema={actionParamSchema}
                      values={rule.data.action_params}
                      emptyMessage="No action parameters configured"
                    />
                  </div>
                )}
            </div>
          </div>
        </div>

        {/* Sidebar */}
        <div className="space-y-6">
          {/* Quick Actions */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Quick Actions</h2>
            <div className="space-y-2">
              <Link
                to={`/packs/${rule.data?.pack_ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Pack
              </Link>
              <Link
                to={`/triggers/${rule.data?.trigger_ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Trigger
              </Link>
              <Link
                to={`/actions/${rule.data?.action_ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Action
              </Link>
              <Link
                to={`/executions?rule=${rule.data?.ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Executions
              </Link>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
