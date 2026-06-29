import { FormEvent, useEffect, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Search } from "lucide-react";
import { useTraceReport } from "@/hooks/useTraceReport";

type TimelineItem = {
  key: string;
  type: "event" | "enforcement" | "execution" | "queue_dispatch" | "queue_item";
  created: string;
  lastActivity?: string | null;
  status?: string;
  title: string;
  subtitle?: string;
  link?: string;
};

function formatTimestamp(value?: string | null): string {
  if (!value) return "-";
  return new Date(value).toLocaleString();
}

function timelineTypeLabel(type: TimelineItem["type"]): string {
  switch (type) {
    case "event":
      return "Event";
    case "enforcement":
      return "Enforcement";
    case "execution":
      return "Execution";
    case "queue_dispatch":
      return "Queue Dispatch";
    case "queue_item":
      return "Queue Item";
  }
}

export default function TraceReportPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const initial = searchParams.get("trace_tag") || "";
  const [traceInput, setTraceInput] = useState(initial);
  const traceTag = useMemo(() => initial.trim(), [initial]);
  const { data, isLoading, error } = useTraceReport(traceTag);
  const report = data?.data;

  useEffect(() => {
    setTraceInput(initial);
  }, [initial]);
  const timeline = useMemo<TimelineItem[]>(() => {
    if (!report) return [];

    const items: TimelineItem[] = [
      ...report.events.map((event) => ({
        key: `event-${event.id}`,
        type: "event" as const,
        created: event.created,
        title: `#${event.id}`,
        subtitle: event.trigger_ref,
        link: `/events/${event.id}`,
      })),
      ...report.enforcements.map((enforcement) => ({
        key: `enforcement-${enforcement.id}`,
        type: "enforcement" as const,
        created: enforcement.created,
        lastActivity: enforcement.resolved_at ?? null,
        status: enforcement.status,
        title: `#${enforcement.id}`,
        subtitle: enforcement.rule_ref,
        link: `/enforcements/${enforcement.id}`,
      })),
      ...report.executions.map((execution) => ({
        key: `execution-${execution.id}`,
        type: "execution" as const,
        created: execution.created,
        lastActivity: execution.updated,
        status: execution.status,
        title: `#${execution.id}`,
        subtitle: execution.action_ref,
        link: `/executions/${execution.id}`,
      })),
      ...report.queue_dispatches.map((dispatch) => ({
        key: `queue-dispatch-${dispatch.id}`,
        type: "queue_dispatch" as const,
        created: dispatch.created,
        lastActivity: dispatch.updated,
        status: dispatch.status,
        title: `#${dispatch.id}`,
        subtitle: `${dispatch.queue_ref} • execution #${dispatch.execution}`,
      })),
      ...report.queue_items.map((item) => ({
        key: `queue-item-${item.id}`,
        type: "queue_item" as const,
        created: item.created,
        lastActivity: item.updated,
        status: item.status,
        title: `#${item.id}`,
        subtitle: `${item.queue_ref}${item.item_key ? ` • ${item.item_key}` : ""}`,
      })),
    ];

    return items.sort(
      (a, b) => new Date(a.created).getTime() - new Date(b.created).getTime(),
    );
  }, [report]);

  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    const next = traceInput.trim();
    if (!next) {
      setSearchParams({});
      return;
    }
    setSearchParams({ trace_tag: next });
  };

  return (
    <div className="p-6 pb-24 space-y-6">
      <div>
        <h1 className="text-3xl font-bold">Trace Report</h1>
        <p className="mt-2 text-gray-600">
          Search by exact trace tag and review related executions, enforcements,
          events, queue dispatches, and queue items.
        </p>
      </div>

      <form
        onSubmit={onSubmit}
        className="bg-white shadow rounded-lg p-4 flex items-end gap-3"
      >
        <div className="flex-1">
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Trace Tag
          </label>
          <input
            value={traceInput}
            onChange={(event) => setTraceInput(event.target.value)}
            placeholder="e.g., core.timer.1234"
            className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <button
          type="submit"
          className="inline-flex items-center gap-2 rounded-md bg-blue-600 px-4 py-2 text-white hover:bg-blue-700"
        >
          <Search className="h-4 w-4" />
          Search
        </button>
      </form>

      {traceTag.length === 0 && (
        <div className="bg-white shadow rounded-lg p-8 text-center text-gray-600">
          Enter a trace tag to generate a report.
        </div>
      )}

      {traceTag.length > 0 && isLoading && (
        <div className="bg-white shadow rounded-lg p-8 text-center text-gray-600">
          Loading trace report...
        </div>
      )}

      {traceTag.length > 0 && error && (
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          {error instanceof Error
            ? error.message
            : "Failed to load trace report"}
        </div>
      )}

      {report && (
        <div className="space-y-4">
          <div className="bg-white shadow rounded-lg p-4">
            <div className="text-sm text-gray-600">
              <span className="font-medium text-gray-900">Trace:</span>{" "}
              <span className="font-mono">{report.trace_tag}</span>
            </div>
            <div className="mt-2 text-sm text-gray-600">
              <span className="font-medium text-gray-900">Origins:</span>{" "}
              {report.origins.length > 0 ? report.origins.join(", ") : "-"}
            </div>
            <div className="mt-3 text-sm text-gray-700">
              Executions {report.executions.length} • Enforcements{" "}
              {report.enforcements.length} • Events {report.events.length} •
              Dispatches {report.queue_dispatches.length} • Queue Items{" "}
              {report.queue_items.length}
            </div>
          </div>

          <div className="bg-white shadow rounded-lg p-4">
            <h2 className="font-semibold mb-3">Timeline</h2>
            {timeline.length === 0 ? (
              <p className="text-sm text-gray-500">
                No related activity found.
              </p>
            ) : (
              <div className="overflow-x-auto">
                <table className="min-w-full text-sm">
                  <thead>
                    <tr className="text-left text-gray-500 border-b">
                      <th className="py-2 pr-4">Created</th>
                      <th className="py-2 pr-4">Type</th>
                      <th className="py-2 pr-4">Item</th>
                      <th className="py-2 pr-4">Status</th>
                      <th className="py-2 pr-0">Last activity</th>
                    </tr>
                  </thead>
                  <tbody>
                    {timeline.map((item) => (
                      <tr key={item.key} className="border-b last:border-b-0">
                        <td className="py-2 pr-4 whitespace-nowrap text-gray-700">
                          {formatTimestamp(item.created)}
                        </td>
                        <td className="py-2 pr-4 whitespace-nowrap text-gray-700">
                          {timelineTypeLabel(item.type)}
                        </td>
                        <td className="py-2 pr-4 text-gray-900">
                          <div className="font-mono">
                            {item.link ? (
                              <Link
                                to={item.link}
                                className="text-blue-600 hover:text-blue-800"
                              >
                                {item.title}
                              </Link>
                            ) : (
                              item.title
                            )}
                          </div>
                          {item.subtitle && (
                            <div className="text-xs text-gray-500">
                              {item.subtitle}
                            </div>
                          )}
                        </td>
                        <td className="py-2 pr-4 whitespace-nowrap text-gray-700">
                          {item.status ?? "-"}
                        </td>
                        <td className="py-2 pr-0 whitespace-nowrap text-gray-700">
                          {item.type === "enforcement" && !item.lastActivity
                            ? "Pending"
                            : formatTimestamp(item.lastActivity)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
