import { AlertTriangle, CheckCircle2, Database } from "lucide-react";
import { PlatformCatalogStatus as CatalogStatus } from "@/api";
import { usePlatformCatalog } from "@/hooks/usePacks";

const statusStyles = {
  [CatalogStatus.CURRENT]: {
    label: "Current",
    className: "bg-green-100 text-green-800",
  },
  [CatalogStatus.UPGRADE_REQUIRED]: {
    label: "Upgrade required",
    className: "bg-yellow-100 text-yellow-800",
  },
  [CatalogStatus.INCOMPATIBLE]: {
    label: "Incompatible",
    className: "bg-red-100 text-red-800",
  },
} satisfies Record<CatalogStatus, { label: string; className: string }>;

export default function PlatformCatalogStatus() {
  const catalog = usePlatformCatalog();
  const state = catalog.data?.data;
  const presentation = state ? statusStyles[state.status] : undefined;

  return (
    <section className="bg-white shadow rounded-lg p-6">
      <div className="mb-4 flex items-center gap-2">
        <Database className="h-5 w-5 text-gray-600" />
        <h2 className="text-lg font-semibold">Platform catalog</h2>
      </div>
      {catalog.isLoading ? (
        <p className="text-sm text-gray-500">Checking catalog...</p>
      ) : catalog.error || !state || !presentation ? (
        <div className="flex items-start gap-2 text-sm text-red-700">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
          Catalog status is unavailable.
        </div>
      ) : (
        <div>
          <span
            className={`inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium ${presentation.className}`}
          >
            {state.status === CatalogStatus.CURRENT ? (
              <CheckCircle2 className="h-3.5 w-3.5" />
            ) : (
              <AlertTriangle className="h-3.5 w-3.5" />
            )}
            {presentation.label}
          </span>
          <dl className="mt-3 grid grid-cols-2 gap-3 text-sm">
            <div>
              <dt className="text-gray-500">Compatibility epoch</dt>
              <dd className="font-mono text-gray-900">
                {state.compatibility_epoch}
              </dd>
            </div>
            <div>
              <dt className="text-gray-500">Catalog revision</dt>
              <dd className="font-mono text-gray-900">{state.revision}</dd>
            </div>
          </dl>
          {state.status !== CatalogStatus.CURRENT && (
            <p className="mt-3 text-xs text-gray-600">
              This API expects epoch {state.expected_compatibility_epoch},
              revision {state.expected_revision}.
            </p>
          )}
        </div>
      )}
    </section>
  );
}
