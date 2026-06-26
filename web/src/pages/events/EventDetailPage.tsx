import { useParams, Link } from "react-router-dom";
import { useEvent } from "@/hooks/useEvents";
import { useTrigger } from "@/hooks/useTriggers";
import { CuratedDataCard } from "@/components/common/CuratedDataPanel";

export default function EventDetailPage() {
  const { id } = useParams<{ id: string }>();
  const eventId = id ? parseInt(id, 10) : 0;

  const { data: eventData, isLoading, error } = useEvent(eventId);
  const event = eventData?.data;
  const eventTraceTag = (
    event as (typeof event & { trace_tag?: string | null }) | undefined
  )?.trace_tag;
  const { data: triggerData } = useTrigger(event?.trigger_ref || "");

  const formatDate = (dateString: string) => {
    return new Date(dateString).toLocaleString();
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

  if (error || !event) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 rounded-lg p-6">
          <h3 className="text-lg font-semibold text-red-900 mb-2">
            Failed to load event
          </h3>
          <p className="text-red-700">
            {error instanceof Error ? error.message : "Event not found"}
          </p>
          <Link
            to="/events"
            className="inline-block mt-4 text-red-600 hover:text-red-800"
          >
            ← Back to Events
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
          to="/events"
          className="text-sm text-blue-600 hover:text-blue-800 mb-2 inline-block"
        >
          ← Back to Events
        </Link>
        <div className="flex items-start justify-between">
          <div>
            <h1 className="text-3xl font-bold text-gray-900">
              Event #{event.id}
            </h1>
            <p className="mt-2 text-gray-600">Trigger: {event.trigger_ref}</p>
            <div className="flex items-center gap-4 mt-3 text-sm text-gray-500">
              <span>ID: {event.id}</span>
              <span>•</span>
              <span>Created: {formatDate(event.created)}</span>
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
                  <dt className="text-sm font-medium text-gray-500">Trigger</dt>
                  <dd className="mt-1">
                    <Link
                      to={`/triggers/${event.trigger_ref}`}
                      className="text-blue-600 hover:text-blue-800"
                    >
                      {event.trigger_ref}
                    </Link>
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">Source</dt>
                  <dd className="mt-1 text-gray-900">
                    {event.source_ref || "N/A"}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">Rule</dt>
                  <dd className="mt-1">
                    {event.rule_ref ? (
                      <Link
                        to={`/rules/${event.rule}`}
                        className="text-blue-600 hover:text-blue-800"
                      >
                        {event.rule_ref}
                      </Link>
                    ) : (
                      <span className="text-gray-500">No rule associated</span>
                    )}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Created At
                  </dt>
                  <dd className="mt-1 text-gray-900">
                    {formatDate(event.created)}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium text-gray-500">Trace Tag</dt>
                  <dd className="mt-1 text-gray-900">
                    {eventTraceTag ? (
                      <Link
                        to={`/traces?trace_tag=${encodeURIComponent(eventTraceTag)}`}
                        className="font-mono text-blue-600 hover:text-blue-800"
                      >
                        {eventTraceTag}
                      </Link>
                    ) : (
                      "N/A"
                    )}
                  </dd>
                </div>
              </dl>
            </div>
          </div>

          <CuratedDataCard
            title="Event Payload"
            description="Payload fields emitted by the trigger, annotated with the trigger output schema when available."
            schema={triggerData?.data?.out_schema}
            values={event.payload}
            emptyMessage="No payload data"
          />
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
              <Link
                to={`/triggers/${event.trigger_ref}`}
                className="block text-sm text-blue-600 hover:text-blue-800"
              >
                → View Trigger: {event.trigger_ref}
              </Link>
              {event.rule_ref && (
                <Link
                  to={`/rules/${event.rule}`}
                  className="block text-sm text-blue-600 hover:text-blue-800"
                >
                  → View Rule: {event.rule_ref}
                </Link>
              )}
              <Link
                to={`/enforcements?event=${event.id}`}
                className="block text-sm text-blue-600 hover:text-blue-800"
              >
                → View Enforcements
              </Link>
              <Link
                to={`/events?trigger_ref=${event.trigger_ref}`}
                className="block text-sm text-blue-600 hover:text-blue-800"
              >
                → View Similar Events
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
                  <dt className="text-gray-500">Event ID</dt>
                  <dd className="text-gray-900 font-mono">{event.id}</dd>
                </div>
                <div>
                  <dt className="text-gray-500">Trigger</dt>
                  <dd className="text-gray-900 font-mono">
                    {event.trigger || "N/A"}
                  </dd>
                </div>
                <div>
                  <dt className="text-gray-500">Trigger Reference</dt>
                  <dd className="text-gray-900">{event.trigger_ref}</dd>
                </div>
                <div>
                  <dt className="text-gray-500">Source</dt>
                  <dd className="text-gray-900">{event.source_ref || "N/A"}</dd>
                </div>
                {event.rule_ref && (
                  <>
                    <div>
                      <dt className="text-gray-500">Rule ID</dt>
                      <dd className="text-gray-900 font-mono">{event.rule}</dd>
                    </div>
                    <div>
                      <dt className="text-gray-500">Rule Reference</dt>
                      <dd className="text-gray-900">{event.rule_ref}</dd>
                    </div>
                  </>
                )}
                <div>
                  <dt className="text-gray-500">Created</dt>
                  <dd className="text-gray-900">{formatDate(event.created)}</dd>
                </div>
                <div>
                  <dt className="text-gray-500">Trace Tag</dt>
                  <dd className="text-gray-900 font-mono">
                    {eventTraceTag || "N/A"}
                  </dd>
                </div>
              </dl>
            </div>
          </div>

          {/* Statistics */}
          <div className="bg-white rounded-lg shadow">
            <div className="px-6 py-4 border-b border-gray-200">
              <h2 className="text-lg font-semibold text-gray-900">
                Statistics
              </h2>
            </div>
            <div className="px-6 py-4">
              <div className="text-sm text-gray-700">
                <p className="mb-2">
                  This event was generated by the{" "}
                  <span className="font-medium">{event.trigger_ref}</span>{" "}
                  trigger
                  {event.rule_ref && (
                    <>
                      {" "}
                      from rule{" "}
                      <span className="font-medium">{event.rule_ref}</span>
                    </>
                  )}
                  .
                </p>
                <p className="text-xs text-gray-500">
                  {event.rule_ref
                    ? "This event is associated with a specific rule and will only trigger that rule's actions."
                    : "Check the enforcements to see if any rules were activated by this event."}
                </p>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
