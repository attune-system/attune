import { Link } from "react-router-dom";
import { ChevronRight, Pencil, Settings2 } from "lucide-react";
import type { WorkQueueResponse } from "@/api/queues";
import {
  formatQueueTunable,
  getBatchModeLabel,
  getQueueSourceBadge,
  parseQueueConfig,
} from "./queueUtils";

interface QueueInspectionPreviewProps {
  queue: WorkQueueResponse;
}

export default function QueueInspectionPreview({
  queue,
}: QueueInspectionPreviewProps) {
  const sourceBadge = getQueueSourceBadge(queue.is_adhoc);
  const config = parseQueueConfig(queue.config);

  return (
    <div className="rounded-lg bg-white p-5 shadow">
      <div>
        <div>
          <div className="inline-flex flex-wrap items-center gap-2 text-sm font-medium text-gray-500">
            <Settings2 className="h-4 w-4" />
            Queue inspection
            <span
              className={`inline-flex rounded-full px-2 py-1 text-xs font-semibold ${sourceBadge.classes}`}
            >
              {sourceBadge.label}
            </span>
          </div>
          <h2 className="mt-2 text-xl font-semibold text-gray-900">
            {queue.label}
          </h2>
          <p className="mt-1 font-mono text-xs text-gray-500">{queue.ref}</p>
        </div>
      </div>

      {queue.description && (
        <p className="mt-4 text-sm text-gray-600">{queue.description}</p>
      )}

      <dl className="mt-5 grid gap-3 text-sm text-gray-700">
        <div className="flex items-start justify-between gap-4">
          <dt className="text-gray-500">Dispatch action</dt>
          <dd className="font-mono text-right text-xs">
            {queue.dispatch_action_ref}
          </dd>
        </div>
        <div className="flex items-start justify-between gap-4">
          <dt className="text-gray-500">Mode</dt>
          <dd>{getBatchModeLabel(queue.batch_mode)}</dd>
        </div>
        <div className="flex items-start justify-between gap-4">
          <dt className="text-gray-500">Concurrency</dt>
          <dd>
            {formatQueueTunable(
              config.dispatch?.concurrency,
              "Default: 1",
              queue.resolved_dispatch_tuning?.concurrency,
            )}
          </dd>
        </div>
        <div className="flex items-start justify-between gap-4">
          <dt className="text-gray-500">Batch size</dt>
          <dd>
            {queue.batch_mode === "batch"
              ? formatQueueTunable(
                  config.dispatch?.batch_size,
                  "Default: 1",
                  queue.resolved_dispatch_tuning?.batch_size,
                )
              : "Single item"}
          </dd>
        </div>
      </dl>

      <div className="mt-6 flex flex-wrap items-center gap-3">
        <Link
          to={`/queues/${encodeURIComponent(queue.ref)}`}
          className="inline-flex items-center gap-2 rounded-lg bg-blue-600 px-3 py-2 text-sm font-medium text-white hover:bg-blue-700"
        >
          Open queue details
          <ChevronRight className="h-4 w-4" />
        </Link>
        {queue.is_adhoc && (
          <Link
            to={`/queues/${encodeURIComponent(queue.ref)}/edit`}
            className="inline-flex items-center gap-2 rounded-lg bg-gray-100 px-3 py-2 text-sm font-medium text-gray-700 hover:bg-gray-200"
          >
            <Pencil className="h-4 w-4" />
            Edit queue
          </Link>
        )}
      </div>
    </div>
  );
}
