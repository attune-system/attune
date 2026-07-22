import { useMemo } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import {
  ArrowLeft,
  Database,
  History,
  ListChecks,
  RefreshCw,
} from "lucide-react";
import { useCacheNamespace } from "@/hooks/useCaches";
import ErrorDisplay from "@/components/common/ErrorDisplay";
import {
  computeNamespaceStatus,
  formatOwnerScope,
  getNamespaceStatusBadge,
  parseOwnerRouteParams,
} from "@/components/caches/cacheUtils";
import CacheOverviewTab from "@/pages/caches/tabs/CacheOverviewTab";
import CacheRecordsTab from "@/pages/caches/tabs/CacheRecordsTab";
import CacheGenerationsTab from "@/pages/caches/tabs/CacheGenerationsTab";
import CacheRefreshTab from "@/pages/caches/tabs/CacheRefreshTab";

type TabKey = "overview" | "records" | "generations" | "refresh";

const TABS: Array<{ key: TabKey; label: string; icon: typeof Database }> = [
  { key: "overview", label: "Overview", icon: Database },
  { key: "records", label: "Records", icon: ListChecks },
  { key: "generations", label: "Generations", icon: History },
  { key: "refresh", label: "Refresh", icon: RefreshCw },
];

export default function CacheNamespaceDetailPage() {
  const { ownerType, ownerRef, namespace } = useParams<{
    ownerType: string;
    ownerRef: string;
    namespace: string;
  }>();
  const [searchParams, setSearchParams] = useSearchParams();
  const activeTab = (searchParams.get("tab") as TabKey) || "overview";

  const owner = useMemo(
    () => parseOwnerRouteParams(ownerType, ownerRef),
    [ownerType, ownerRef],
  );

  const setTab = (tab: TabKey) => {
    const next = new URLSearchParams(searchParams);
    next.set("tab", tab);
    setSearchParams(next);
  };

  const namespaceQuery = useCacheNamespace(owner ?? undefined, namespace);

  if (!owner || !namespace) {
    return (
      <div className="p-6">
        <ErrorDisplay
          error={new Error("Invalid cache namespace route")}
          title="Invalid cache namespace"
        />
      </div>
    );
  }

  const data = namespaceQuery.data?.data;
  const status = data ? computeNamespaceStatus(data) : undefined;
  const badge = status ? getNamespaceStatusBadge(status) : undefined;

  return (
    <div className="p-6 pb-28">
      <div className="mb-4">
        <Link
          to="/caches"
          className="inline-flex items-center gap-1 text-sm text-gray-500 hover:text-gray-700"
        >
          <ArrowLeft className="h-4 w-4" />
          Back to Data Caches
        </Link>
      </div>

      <div className="mb-6 flex items-center justify-between">
        <div>
          <h1 className="flex items-center gap-3 text-3xl font-bold text-gray-900">
            <Database className="h-7 w-7 text-teal-600" />
            <span className="font-mono">{namespace}</span>
            {badge && (
              <span
                className={`inline-flex rounded-full px-2.5 py-1 text-xs font-semibold leading-5 ${badge.classes}`}
              >
                {badge.label}
              </span>
            )}
          </h1>
          <p className="mt-2 text-gray-600">
            {data ? formatOwnerScope(data) : "Loading owner scope…"}
          </p>
        </div>
      </div>

      {namespaceQuery.error ? (
        <ErrorDisplay
          error={namespaceQuery.error}
          title="Failed to load cache namespace"
        />
      ) : (
        <>
          <div className="mb-6 border-b border-gray-200">
            <nav className="-mb-px flex space-x-8">
              {TABS.map(({ key, label, icon: Icon }) => (
                <button
                  key={key}
                  onClick={() => setTab(key)}
                  className={`whitespace-nowrap border-b-2 px-1 py-3 text-sm font-medium transition-colors ${
                    activeTab === key
                      ? "border-teal-500 text-teal-600"
                      : "border-transparent text-gray-500 hover:border-gray-300 hover:text-gray-700"
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <Icon className="h-4 w-4" />
                    {label}
                  </div>
                </button>
              ))}
            </nav>
          </div>

          {namespaceQuery.isLoading || !data ? (
            <div className="p-12 text-center">
              <div className="mx-auto inline-block h-8 w-8 animate-spin rounded-full border-b-2 border-teal-600"></div>
              <p className="mt-4 text-gray-600">Loading namespace…</p>
            </div>
          ) : (
            <>
              {activeTab === "overview" && (
                <CacheOverviewTab owner={owner} namespace={data} />
              )}
              {activeTab === "records" && (
                <CacheRecordsTab owner={owner} namespaceName={namespace} />
              )}
              {activeTab === "generations" && (
                <CacheGenerationsTab owner={owner} namespaceName={namespace} />
              )}
              {activeTab === "refresh" && (
                <CacheRefreshTab
                  owner={owner}
                  namespaceName={namespace}
                  namespace={data}
                />
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}
