import { ArrowLeft } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import type { JsonValue } from "@/api/queues";
import {
  formatDateTime,
  formatJsonPreview,
  getStatusBadge,
} from "@/components/queues/queueUtils";
import { useQueueItem } from "@/hooks/useQueues";

export default function QueueItemDetailPage() {
  const { ref = "", itemId = "" } = useParams<{
    ref: string;
    itemId: string;
  }>();
  const parsedItemId = /^\d+$/.test(itemId) ? Number(itemId) : null;
  const { data, isLoading, error } = useQueueItem(ref, parsedItemId);
  const item = data?.data;

  return (
    <div className="mx-auto max-w-5xl p-6">
      <Link
        to={`/queues/${encodeURIComponent(ref)}`}
        className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900"
      >
        <ArrowLeft className="mr-1 h-4 w-4" />
        Back to queue
      </Link>

      {isLoading ? (
        <div className="flex h-64 items-center justify-center">
          <div className="h-12 w-12 animate-spin rounded-full border-b-2 border-blue-600" />
        </div>
      ) : error || !item ? (
        <div className="mt-6 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-red-700">
          {error instanceof Error ? error.message : "Queue item not found"}
        </div>
      ) : (
        <div className="mt-6 space-y-6">
          <div>
            <h1 className="text-3xl font-bold text-gray-900">
              Queue item #{item.id}
            </h1>
            <p className="mt-2 font-mono text-sm text-gray-500">
              {item.queue_ref}
            </p>
          </div>

          <div className="rounded-lg bg-white p-5 shadow">
            <div className="grid gap-5 md:grid-cols-2 xl:grid-cols-4">
              <Detail
                label="Status"
                value={getStatusBadge(item.status).label}
              />
              <Detail label="Item key" value={item.item_key ?? "None"} mono />
              <Detail label="Attempts" value={String(item.attempt_count)} />
              <Detail label="Updated" value={formatDateTime(item.updated)} />
            </div>
          </div>

          <JsonCard
            title="Result"
            value={item.ack_summary ?? item.last_error ?? null}
          />
          <JsonCard title="Payload" value={item.payload} />
          <JsonCard title="Metadata" value={item.metadata} />
        </div>
      )}
    </div>
  );
}

function Detail({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div>
      <div className="text-xs font-medium uppercase tracking-wide text-gray-500">
        {label}
      </div>
      <div
        className={`mt-1 break-all text-sm text-gray-900 ${mono ? "font-mono" : ""}`}
      >
        {value}
      </div>
    </div>
  );
}

function JsonCard({ title, value }: { title: string; value: JsonValue }) {
  return (
    <div className="rounded-lg bg-white p-5 shadow">
      <h2 className="text-lg font-semibold text-gray-900">{title}</h2>
      <pre className="mt-3 whitespace-pre-wrap break-words text-xs text-gray-700">
        {formatJsonPreview(value)}
      </pre>
    </div>
  );
}
