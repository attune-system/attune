import type {
  DashboardSourceResult,
  DashboardSourceStatus,
} from "@/types/dashboard";

function badgeClass(status: DashboardSourceStatus): string {
  switch (status) {
    case "ok":
      return "bg-green-100 text-green-700";
    case "empty":
      return "bg-gray-100 text-gray-700";
    case "partial":
      return "bg-yellow-100 text-yellow-800";
    case "stale":
      return "bg-amber-100 text-amber-800";
    case "forbidden":
      return "bg-purple-100 text-purple-800";
    case "invalid":
      return "bg-orange-100 text-orange-800";
    case "error":
      return "bg-red-100 text-red-700";
    default:
      return "bg-gray-100 text-gray-700";
  }
}

interface DashboardPreviewStatusListProps {
  sources: DashboardSourceResult[];
}

export function DashboardPreviewStatusList({
  sources,
}: DashboardPreviewStatusListProps) {
  if (!sources.length) {
    return <p className="text-sm text-gray-500">No preview sources yet.</p>;
  }

  return (
    <div className="space-y-2">
      {sources.map((source) => (
        <div
          key={source.source_id}
          className="rounded border border-gray-200 bg-white px-3 py-2"
        >
          <div className="flex items-center justify-between gap-3">
            <div>
              <p className="text-sm font-medium text-gray-900">
                {source.source_id}
              </p>
              <p className="text-xs text-gray-500">{source.source_type}</p>
            </div>
            <span
              className={`rounded-full px-2 py-0.5 text-[10px] ${badgeClass(source.status)}`}
            >
              {source.status}
            </span>
          </div>
          <div className="mt-2 flex flex-wrap gap-2 text-[11px] text-gray-600">
            <span>freshness: {source.meta.freshness_mode}</span>
            {source.meta.bucket_size && (
              <span>bucket: {source.meta.bucket_size}</span>
            )}
            {source.meta.aggregate_watermark && (
              <span>watermark: {source.meta.aggregate_watermark}</span>
            )}
            {source.meta.truncated && (
              <span className="text-amber-700">truncated</span>
            )}
          </div>
          {source.error && (
            <p className="mt-2 text-xs text-red-700">
              {source.error.message}
              {source.error.code ? ` (${source.error.code})` : ""}
            </p>
          )}
        </div>
      ))}
    </div>
  );
}
