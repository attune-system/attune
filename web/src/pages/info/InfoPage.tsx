import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { InfoService, OpenAPI } from "@/api";

export default function InfoPage() {
  const query = useQuery({
    queryKey: ["server-info", OpenAPI.BASE],
    queryFn: () => InfoService.getInfo(),
    staleTime: 0,
    retry: false,
  });

  return (
    <div className="p-6 max-w-4xl mx-auto">
      <div className="flex items-center justify-between gap-4 mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-gray-900">System info</h1>
          <p className="mt-1 text-sm text-gray-600">
            Build identity reported by the API server handling this request.
          </p>
        </div>
        <button
          type="button"
          onClick={() => void query.refetch()}
          disabled={query.isFetching}
          className="inline-flex items-center gap-2 px-3 py-2 border border-gray-300 rounded-md text-sm disabled:opacity-50"
        >
          <RefreshCw className="h-4 w-4" />
          Refresh
        </button>
      </div>
      <section className="rounded-lg bg-white p-6 shadow border border-gray-200">
        <h2 className="text-lg font-semibold text-gray-900 mb-4">
          Server build
        </h2>
        <dl className="space-y-4">
          <div>
            <dt className="text-sm text-gray-500">API origin</dt>
            <dd className="mt-1 font-mono break-all">
              {new URL(OpenAPI.BASE, window.location.origin).origin}
            </dd>
          </div>
          {query.data && (
            <>
              <div>
                <dt className="text-sm text-gray-500">Semantic version</dt>
                <dd className="mt-1 text-xl font-semibold">
                  {query.data.data.version}
                </dd>
              </div>
              <div>
                <dt className="text-sm text-gray-500">Git SHA</dt>
                <dd className="mt-1 font-mono break-all">
                  {query.data.data.git_sha}
                </dd>
              </div>
            </>
          )}
        </dl>
        {query.isPending && (
          <p className="mt-4 text-gray-500">
            Fetching server build information...
          </p>
        )}
        {query.error && (
          <div
            role="alert"
            className="mt-4 rounded-md border border-red-200 bg-red-50 p-3 text-red-700"
          >
            Could not fetch server build information. {query.error.message}
          </div>
        )}
      </section>
      <p className="mt-4 text-sm text-gray-500">
        A Git SHA of "unknown" means the build did not include source revision
        metadata. During a rolling deployment, refresh may reach a different API
        replica.
      </p>
    </div>
  );
}
