import { Link } from "react-router-dom";
import { Activity, ArrowRight, Clock3 } from "lucide-react";
import { WorkQueueItemStatus } from "@/api/queues";
import { useQueueItems } from "@/hooks/useQueues";
import {
  formatDateTime,
  formatJsonPreview,
  getStatusBadge,
} from "./queueUtils";

interface QueueUpNextListProps {
  queueRef: string;
  pageSize?: number;
}

export default function QueueUpNextList({
  queueRef,
  pageSize = 8,
}: QueueUpNextListProps) {
  const { data, isLoading, error, isFetching } = useQueueItems(queueRef, {
    page: 1,
    pageSize,
    statuses: [WorkQueueItemStatus.QUEUED, WorkQueueItemStatus.RETRY],
  });

  const items = data?.items ?? [];
  const total = data?.pagination?.total_items ?? 0;

  return (
    <div className="rounded-lg bg-white p-5 shadow">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-lg font-semibold text-gray-900">Up next</h2>
          <p className="mt-1 text-sm text-gray-500">
            Live queued and retry items waiting to dispatch.
          </p>
        </div>
        <div className="inline-flex items-center gap-2 rounded-full bg-emerald-50 px-3 py-1 text-xs font-medium text-emerald-700">
          <Activity className="h-3.5 w-3.5" />
          Streaming
        </div>
      </div>

      <div className="mt-3 text-sm text-gray-600">
        {total > 0
          ? `${total} pending item${total === 1 ? "" : "s"}`
          : "No pending items"}
        {isFetching && !isLoading ? " • refreshing…" : ""}
      </div>

      {isLoading ? (
        <div className="py-10 text-center">
          <div className="inline-block h-7 w-7 animate-spin rounded-full border-b-2 border-blue-600" />
          <p className="mt-3 text-sm text-gray-600">Loading queue items...</p>
        </div>
      ) : error ? (
        <div className="mt-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error instanceof Error
            ? error.message
            : "Failed to load queue items"}
        </div>
      ) : items.length === 0 ? (
        <div className="mt-4 rounded-lg border border-dashed border-gray-300 px-5 py-10 text-center">
          <Clock3 className="mx-auto h-8 w-8 text-gray-400" />
          <p className="mt-3 text-sm text-gray-600">
            Nothing is waiting in this queue right now.
          </p>
        </div>
      ) : (
        <div className="mt-4 space-y-3">
          {items.map((item) => {
            const statusBadge = getStatusBadge(item.status);
            return (
              <div
                key={item.id}
                className="rounded-lg border border-gray-200 bg-gray-50 p-4"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="text-sm font-semibold text-gray-900">
                      #{item.id}
                      {item.item_key ? ` • ${item.item_key}` : ""}
                    </div>
                    <div className="mt-1 text-xs text-gray-500">
                      Priority {item.priority} • {item.enqueue_source}
                    </div>
                  </div>
                  <span
                    className={`inline-flex rounded-full px-2 py-1 text-xs font-semibold ${statusBadge.classes}`}
                  >
                    {statusBadge.label}
                  </span>
                </div>
                <div className="mt-3 text-xs text-gray-700">
                  Payload: {formatJsonPreview(item.payload, 160)}
                </div>
                <div className="mt-2 text-xs text-gray-500">
                  Created {formatDateTime(item.created)}
                </div>
              </div>
            );
          })}
        </div>
      )}

      <div className="mt-4">
        <Link
          to={`/queues/${encodeURIComponent(queueRef)}`}
          className="inline-flex items-center gap-2 text-sm font-medium text-blue-600 hover:text-blue-800"
        >
          Open full queue detail
          <ArrowRight className="h-4 w-4" />
        </Link>
      </div>
    </div>
  );
}
