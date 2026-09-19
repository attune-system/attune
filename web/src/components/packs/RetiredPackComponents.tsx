import { ChevronDown, History } from "lucide-react";
import { useState } from "react";
import { useRetiredPackComponents } from "@/hooks/usePacks";

function componentKindLabel(kind: string): string {
  return kind.replaceAll("_", " ");
}

export default function RetiredPackComponents({
  packRef,
}: {
  packRef: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const components = useRetiredPackComponents(packRef, expanded);

  return (
    <section className="bg-white shadow rounded-lg p-6">
      <button
        type="button"
        onClick={() => setExpanded((current) => !current)}
        aria-expanded={expanded}
        className="flex w-full items-center justify-between text-left"
      >
        <span className="flex items-center gap-2">
          <History className="h-5 w-5 text-gray-600" />
          <span className="text-lg font-semibold">Retired components</span>
        </span>
        <ChevronDown
          className={`h-4 w-4 text-gray-500 transition-transform ${expanded ? "rotate-180" : ""}`}
        />
      </button>
      {expanded && (
        <div className="mt-4 border-t border-gray-200 pt-4">
          {components.isLoading ? (
            <p className="text-sm text-gray-500">
              Loading retired components...
            </p>
          ) : components.error ? (
            <p className="text-sm text-red-700">
              Retired components are unavailable.
            </p>
          ) : components.data?.data.length === 0 ? (
            <p className="text-sm text-gray-500">
              No components have been retired.
            </p>
          ) : (
            <ul className="space-y-3">
              {components.data?.data.map((component) => (
                <li key={`${component.kind}-${component.id}`}>
                  <div className="flex items-center justify-between gap-2">
                    <code className="truncate text-xs text-gray-800">
                      {component.component_ref ?? `#${component.id}`}
                    </code>
                    <span className="shrink-0 rounded bg-gray-100 px-2 py-0.5 text-xs capitalize text-gray-700">
                      {componentKindLabel(component.kind)}
                    </span>
                  </div>
                  <div className="mt-1 flex justify-between text-xs text-gray-500">
                    <span>
                      {new Date(component.retired_at).toLocaleString()}
                    </span>
                    {component.managed_release != null && (
                      <span>release #{component.managed_release}</span>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
