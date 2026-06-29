import { useParams, Link } from "react-router-dom";
import { useEnforcement } from "@/hooks/useEvents";
import { useTrigger } from "@/hooks/useTriggers";
import { useRule } from "@/hooks/useRules";
import { useAction } from "@/hooks/useActions";
import { EnforcementStatus, EnforcementCondition } from "@/api";
import { CuratedDataCard } from "@/components/common/CuratedDataPanel";

export default function EnforcementDetailPage() {
  const { id } = useParams<{ id: string }>();
  const enforcementId = id ? parseInt(id, 10) : 0;

  const {
    data: enforcementData,
    isLoading,
    error,
  } = useEnforcement(enforcementId);
  const enforcement = enforcementData?.data;
  const enforcementTraceTag = (
    enforcement as
      (typeof enforcement & { trace_tag?: string | null }) | undefined
  )?.trace_tag;
  const { data: triggerData } = useTrigger(enforcement?.trigger_ref || "");
  const { data: ruleData } = useRule(enforcement?.rule_ref || "");
  const { data: actionData } = useAction(ruleData?.data?.action_ref || "");

  const formatDate = (dateString: string) => {
    return new Date(dateString).toLocaleString();
  };

  const getStatusColor = (status: EnforcementStatus) => {
    switch (status) {
      case EnforcementStatus.PROCESSED:
        return "bg-green-100 text-green-800";
      case EnforcementStatus.DISABLED:
        return "bg-gray-100 text-gray-800";
      case EnforcementStatus.CREATED:
        return "bg-blue-100 text-blue-800";
      default:
        return "bg-gray-100 text-gray-800";
    }
  };

  const getConditionColor = (condition: EnforcementCondition) => {
    switch (condition) {
      case EnforcementCondition.ALL:
        return "bg-purple-100 text-purple-800";
      case EnforcementCondition.ANY:
        return "bg-indigo-100 text-indigo-800";
      default:
        return "bg-gray-100 text-gray-800";
    }
  };

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
        </div>
      </div>
    );
  }

  if (error || !enforcement) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 rounded-lg p-6">
          <h3 className="text-lg font-semibold text-red-900 mb-2">
            Failed to load enforcement
          </h3>
          <p className="text-red-700">
            {error instanceof Error ? error.message : "Enforcement not found"}
          </p>
          <Link
            to="/enforcements"
            className="inline-block mt-4 text-red-600 hover:text-red-800"
          >
            ← Back to Enforcements
          </Link>
        </div>
      </div>
    );
  }

  return (
    <div className="p-6">
      {/* Header */}
      <div className="mb-6">
        <Link
          to="/enforcements"
          className="text-sm text-blue-600 hover:text-blue-800 mb-2 inline-block"
        >
          ← Back to Enforcements
        </Link>
        <div className="flex items-start justify-between">
          <div>
            <h1 className="text-3xl font-bold text-gray-900">
              Enforcement #{enforcement.id}
            </h1>
            <p className="mt-2 text-gray-600">Rule: {enforcement.rule_ref}</p>
            <div className="flex items-center gap-4 mt-3">
              <span
                className={`px-3 py-1 text-sm font-semibold rounded-full ${getStatusColor(enforcement.status)}`}
              >
                {enforcement.status}
              </span>
              <span
                className={`px-3 py-1 text-sm font-semibold rounded-full ${getConditionColor(enforcement.condition)}`}
              >
                Condition: {enforcement.condition}
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* Main Content Grid */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Left Column - Main Info */}
        <div className="lg:col-span-2 space-y-6">
          {/* Overview Card */}
          <div className="bg-white rounded-lg shadow">
            <div className="px-6 py-4 border-b border-gray-200">
              <h2 className="text-lg font-semibold text-gray-900">Overview</h2>
            </div>
            <div className="px-6 py-4">
              <dl className="grid grid-cols-1 gap-4">
                <div>
                  <dt className="text-sm font-medium text-gray-500">Rule</dt>
                  <dd className="mt-1">
                    {enforcement.rule ? (
                      <Link
                        to={`/rules/${enforcement.rule}`}
                        className="text-blue-600 hover:text-blue-800"
                      >
                        {enforcement.rule_ref}
                      </Link>
                    ) : (
                      <span className="text-gray-900">
                        {enforcement.rule_ref}
                      </span>
                    )}
                    {enforcement.rule && (
                      <span className="ml-2 text-sm text-gray-500">
                        (ID: {enforcement.rule})
                      </span>
                    )}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">Trigger</dt>
                  <dd className="mt-1 text-gray-900">
                    {enforcement.trigger_ref}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">Event</dt>
                  <dd className="mt-1">
                    {enforcement.event ? (
                      <Link
                        to={`/events/${enforcement.event}`}
                        className="text-blue-600 hover:text-blue-800 font-mono"
                      >
                        #{enforcement.event}
                      </Link>
                    ) : (
                      <span className="text-gray-500">No event associated</span>
                    )}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">Status</dt>
                  <dd className="mt-1">
                    <span
                      className={`px-2 py-1 text-xs font-semibold rounded-full ${getStatusColor(enforcement.status)}`}
                    >
                      {enforcement.status}
                    </span>
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Condition Type
                  </dt>
                  <dd className="mt-1">
                    <span
                      className={`px-2 py-1 text-xs font-semibold rounded-full ${getConditionColor(enforcement.condition)}`}
                    >
                      {enforcement.condition}
                    </span>
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Created At
                  </dt>
                  <dd className="mt-1 text-gray-900">
                    {formatDate(enforcement.created)}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Resolved At
                  </dt>
                  <dd className="mt-1 text-gray-900">
                    {enforcement.resolved_at ? (
                      formatDate(enforcement.resolved_at)
                    ) : (
                      <span className="text-gray-500">Pending</span>
                    )}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Trace Tag
                  </dt>
                  <dd className="mt-1 text-gray-900">
                    {enforcementTraceTag ? (
                      <Link
                        to={`/traces?trace_tag=${encodeURIComponent(enforcementTraceTag)}`}
                        className="font-mono text-blue-600 hover:text-blue-800"
                      >
                        {enforcementTraceTag}
                      </Link>
                    ) : (
                      "N/A"
                    )}
                  </dd>
                </div>
              </dl>
            </div>
          </div>

          {/* Conditions Card */}
          {enforcement.conditions &&
            Object.keys(enforcement.conditions).length > 0 && (
              <CuratedDataCard
                title="Rule Conditions"
                description="Condition values evaluated for this enforcement."
                values={enforcement.conditions}
                emptyMessage="No condition details were captured."
              />
            )}

          {/* Configuration Card */}
          {enforcement.config && Object.keys(enforcement.config).length > 0 && (
            <CuratedDataCard
              title="Enforcement Configuration"
              description="Action parameters resolved for this enforcement, annotated with the action parameter schema when available."
              schema={actionData?.data?.param_schema}
              values={enforcement.config}
              emptyMessage="No enforcement configuration was captured."
              maskSecrets
            />
          )}

          {/* Payload Card */}
          {enforcement.payload &&
            Object.keys(enforcement.payload).length > 0 && (
              <CuratedDataCard
                title="Payload"
                description="Trigger payload that activated the rule, annotated with the trigger output schema when available."
                schema={triggerData?.data?.out_schema}
                values={enforcement.payload}
                emptyMessage="No payload data was captured."
              />
            )}
        </div>

        {/* Right Column - Quick Links and Info */}
        <div className="space-y-6">
          {/* Quick Links */}
          <div className="bg-white rounded-lg shadow">
            <div className="px-6 py-4 border-b border-gray-200">
              <h2 className="text-lg font-semibold text-gray-900">
                Quick Links
              </h2>
            </div>
            <div className="px-6 py-4 space-y-2">
              {enforcement.rule && (
                <Link
                  to={`/rules/${enforcement.rule}`}
                  className="block text-sm text-blue-600 hover:text-blue-800"
                >
                  → View Rule: {enforcement.rule_ref}
                </Link>
              )}
              {enforcement.event && (
                <Link
                  to={`/events/${enforcement.event}`}
                  className="block text-sm text-blue-600 hover:text-blue-800"
                >
                  → View Event #{enforcement.event}
                </Link>
              )}
              <Link
                to={`/triggers/${enforcement.trigger_ref}`}
                className="block text-sm text-blue-600 hover:text-blue-800"
              >
                → View Trigger: {enforcement.trigger_ref}
              </Link>
              <Link
                to={`/executions?enforcement=${enforcement.id}`}
                className="block text-sm text-blue-600 hover:text-blue-800"
              >
                → View Related Executions
              </Link>
              <Link
                to={`/enforcements?rule=${enforcement.rule}`}
                className="block text-sm text-blue-600 hover:text-blue-800"
              >
                → View Similar Enforcements
              </Link>
            </div>
          </div>

          {/* Metadata */}
          <div className="bg-white rounded-lg shadow">
            <div className="px-6 py-4 border-b border-gray-200">
              <h2 className="text-lg font-semibold text-gray-900">Metadata</h2>
            </div>
            <div className="px-6 py-4">
              <dl className="space-y-3 text-sm">
                <div>
                  <dt className="text-gray-500">Enforcement ID</dt>
                  <dd className="text-gray-900 font-mono">{enforcement.id}</dd>
                </div>
                <div>
                  <dt className="text-gray-500">Rule ID</dt>
                  <dd className="text-gray-900 font-mono">
                    {enforcement.rule || "N/A"}
                  </dd>
                </div>
                <div>
                  <dt className="text-gray-500">Rule Reference</dt>
                  <dd className="text-gray-900">{enforcement.rule_ref}</dd>
                </div>
                <div>
                  <dt className="text-gray-500">Event ID</dt>
                  <dd className="text-gray-900 font-mono">
                    {enforcement.event || "N/A"}
                  </dd>
                </div>
                <div>
                  <dt className="text-gray-500">Trigger Reference</dt>
                  <dd className="text-gray-900">{enforcement.trigger_ref}</dd>
                </div>
                <div>
                  <dt className="text-gray-500">Created</dt>
                  <dd className="text-gray-900">
                    {formatDate(enforcement.created)}
                  </dd>
                </div>
                {enforcement.resolved_at && (
                  <div>
                    <dt className="text-gray-500">Resolved</dt>
                    <dd className="text-gray-900">
                      {formatDate(enforcement.resolved_at)}
                    </dd>
                  </div>
                )}
                <div>
                  <dt className="text-gray-500">Trace Tag</dt>
                  <dd className="text-gray-900 font-mono">
                    {enforcementTraceTag || "N/A"}
                  </dd>
                </div>
              </dl>
            </div>
          </div>

          {/* Information */}
          <div className="bg-white rounded-lg shadow">
            <div className="px-6 py-4 border-b border-gray-200">
              <h2 className="text-lg font-semibold text-gray-900">
                About Enforcements
              </h2>
            </div>
            <div className="px-6 py-4">
              <div className="text-sm text-gray-700 space-y-3">
                <p>
                  An enforcement represents the activation of a rule in response
                  to a trigger event. It tracks the conditions that were
                  evaluated and the parameters used for execution.
                </p>
                <div className="bg-blue-50 border border-blue-200 rounded p-3">
                  <p className="font-medium text-blue-900 mb-1">
                    Condition Type: {enforcement.condition}
                  </p>
                  <p className="text-xs text-blue-700">
                    {enforcement.condition === EnforcementCondition.ALL
                      ? "All conditions must be satisfied for this enforcement to execute."
                      : "Any condition can be satisfied for this enforcement to execute."}
                  </p>
                </div>
                <div className="bg-gray-50 border border-gray-200 rounded p-3">
                  <p className="font-medium text-gray-900 mb-1">
                    Status: {enforcement.status}
                  </p>
                  <p className="text-xs text-gray-700">
                    {enforcement.status === EnforcementStatus.CREATED &&
                      "This enforcement has been created and is awaiting processing."}
                    {enforcement.status === EnforcementStatus.PROCESSED &&
                      "This enforcement has been processed and actions have been executed."}
                    {enforcement.status === EnforcementStatus.DISABLED &&
                      "This enforcement has been disabled and will not trigger actions."}
                  </p>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
