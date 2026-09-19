import { Archive, CheckCircle2 } from "lucide-react";
import { usePackReleases } from "@/hooks/usePacks";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export default function PackReleaseHistory({ packRef }: { packRef: string }) {
  const releases = usePackReleases(packRef);

  return (
    <section className="bg-white shadow rounded-lg p-6">
      <div className="mb-4 flex items-center gap-2">
        <Archive className="h-5 w-5 text-gray-600" />
        <h2 className="text-xl font-semibold">Release history</h2>
      </div>
      {releases.isLoading ? (
        <p className="text-sm text-gray-500">Loading releases...</p>
      ) : releases.error ? (
        <p className="text-sm text-red-700">Release history is unavailable.</p>
      ) : releases.data?.data.length === 0 ? (
        <p className="text-sm text-gray-500">No immutable releases recorded.</p>
      ) : (
        <ol className="divide-y divide-gray-200">
          {releases.data?.data.map((release) => (
            <li key={release.id} className="py-3 first:pt-0 last:pb-0">
              <div className="flex flex-col items-start gap-2 sm:flex-row sm:justify-between sm:gap-4">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-medium text-gray-900">
                      v{release.version}
                    </span>
                    {release.is_active && (
                      <span className="inline-flex items-center gap-1 rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-800">
                        <CheckCircle2 className="h-3 w-3" />
                        Active
                      </span>
                    )}
                  </div>
                  <code
                    className="mt-1 block truncate text-xs text-gray-500"
                    title={release.digest}
                  >
                    {release.digest}
                  </code>
                </div>
                <div className="shrink-0 text-left text-xs text-gray-500 sm:text-right">
                  <div>{new Date(release.created).toLocaleString()}</div>
                  <div className="mt-1">
                    {formatBytes(release.archive_size)}
                  </div>
                </div>
              </div>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
