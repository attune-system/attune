import type { ReactNode } from "react";
import { Key, Package, Shield } from "lucide-react";
import {
  ACTION_STYLE,
  RESOURCE_META,
  type GrantConstraints,
  type ParsedGrant,
} from "@/components/access-control/grants";

export function ConstraintChips({ c }: { c: GrantConstraints }) {
  const chips: ReactNode[] = [];

  if (c.pack_refs?.length) {
    chips.push(
      <span
        key="pack_refs"
        className="inline-flex items-center gap-1 rounded border border-green-200 bg-green-50 px-1.5 py-0.5 text-xs text-green-700"
      >
        <Package className="h-3 w-3 shrink-0" />
        {c.pack_refs.join(", ")}
      </span>,
    );
  }

  if (c.owner) {
    const labels: Record<string, string> = {
      self: "Own resources",
      any: "Any owner",
      none: "No owner",
    };
    chips.push(
      <span
        key="owner"
        className="inline-flex items-center rounded border border-blue-200 bg-blue-50 px-1.5 py-0.5 text-xs text-blue-700"
      >
        Owner: {labels[c.owner] ?? c.owner}
      </span>,
    );
  }

  if (c.owner_types?.length) {
    chips.push(
      <span
        key="owner_types"
        className="inline-flex items-center rounded border border-slate-200 bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600"
      >
        Type: {c.owner_types.join(", ")}
      </span>,
    );
  }

  if (c.visibility?.length) {
    chips.push(
      <span
        key="visibility"
        className="inline-flex items-center rounded border border-sky-200 bg-sky-50 px-1.5 py-0.5 text-xs text-sky-700"
      >
        Visibility: {c.visibility.join(", ")}
      </span>,
    );
  }

  if (c.execution_scope) {
    const labels: Record<string, string> = {
      self: "Own executions",
      descendants: "Own + children",
      any: "All executions",
    };
    chips.push(
      <span
        key="execution_scope"
        className="inline-flex items-center rounded border border-purple-200 bg-purple-50 px-1.5 py-0.5 text-xs text-purple-700"
      >
        Scope: {labels[c.execution_scope] ?? c.execution_scope}
      </span>,
    );
  }

  if (c.refs?.length) {
    chips.push(
      <span
        key="refs"
        className="inline-flex items-center rounded border border-slate-200 bg-slate-100 px-1.5 py-0.5 font-mono text-xs text-slate-600"
      >
        {c.refs.join(", ")}
      </span>,
    );
  }

  if (c.encrypted !== undefined) {
    chips.push(
      <span
        key="encrypted"
        className="inline-flex items-center gap-1 rounded border border-amber-200 bg-amber-50 px-1.5 py-0.5 text-xs text-amber-700"
      >
        <Key className="h-3 w-3 shrink-0" />
        {c.encrypted ? "Encrypted only" : "Unencrypted only"}
      </span>,
    );
  }

  if (c.attributes && Object.keys(c.attributes).length > 0) {
    const text = Object.entries(c.attributes)
      .map(([key, value]) => `${key} = ${JSON.stringify(value)}`)
      .join(", ");
    chips.push(
      <span
        key="attributes"
        className="inline-flex items-center rounded border border-rose-200 bg-rose-50 px-1.5 py-0.5 font-mono text-xs text-rose-700"
      >
        {text}
      </span>,
    );
  }

  if (chips.length === 0) {
    return <span className="text-xs text-gray-300">—</span>;
  }

  return <div className="flex flex-col gap-1">{chips}</div>;
}

interface GrantsViewProps {
  grants: ParsedGrant[];
  emptyStateTitle?: string;
  emptyStateDescription?: string;
  sourceColumnTitle?: string;
  renderSource?: (grant: ParsedGrant, index: number) => ReactNode;
  scrollClassName?: string;
}

export function GrantsView({
  grants,
  emptyStateTitle = "No grants defined",
  emptyStateDescription,
  sourceColumnTitle,
  renderSource,
  scrollClassName = "max-h-[28rem] overflow-y-auto",
}: GrantsViewProps) {
  if (grants.length === 0) {
    return (
      <div className="p-8 text-center">
        <Shield className="mx-auto h-8 w-8 text-gray-300" />
        <p className="mt-2 text-sm text-gray-500">{emptyStateTitle}</p>
        {emptyStateDescription && (
          <p className="mt-1 text-xs text-gray-400">{emptyStateDescription}</p>
        )}
      </div>
    );
  }

  const hasConstraints = grants.some(
    (grant) => grant.constraints && Object.keys(grant.constraints).length > 0,
  );
  const hasSources = Boolean(renderSource && sourceColumnTitle);

  return (
    <div className={scrollClassName}>
      <table className="min-w-full">
        <thead className="sticky top-0 z-10 border-b border-gray-200 bg-gray-50">
          <tr>
            <th className="w-36 px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Resource
            </th>
            <th className="px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Permissions
            </th>
            {hasConstraints && (
              <th className="px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                Conditions
              </th>
            )}
            {hasSources && (
              <th className="px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                {sourceColumnTitle}
              </th>
            )}
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-100">
          {grants.map((grant, index) => {
            const meta = RESOURCE_META[grant.resource];
            const Icon = meta?.icon ?? Shield;
            const iconColor = meta?.color ?? "text-gray-400";
            const label =
              meta?.label ??
              grant.resource.charAt(0).toUpperCase() + grant.resource.slice(1);

            return (
              <tr key={`${grant.resource}-${index}`} className="hover:bg-gray-50">
                <td className="whitespace-nowrap px-4 py-2.5 align-top">
                  <div className="flex items-center gap-1.5">
                    <Icon className={`h-3.5 w-3.5 shrink-0 ${iconColor}`} />
                    <span className="text-sm font-medium text-gray-800">
                      {label}
                    </span>
                  </div>
                </td>

                <td className="px-4 py-2.5 align-top">
                  <div className="flex flex-wrap gap-1">
                    {grant.actions.map((action) => (
                      <span
                        key={action}
                        className={`inline-flex items-center rounded px-1.5 py-0.5 text-xs font-medium ${
                          ACTION_STYLE[action] ?? "bg-gray-100 text-gray-700"
                        }`}
                      >
                        {action}
                      </span>
                    ))}
                  </div>
                </td>

                {hasConstraints && (
                  <td className="px-4 py-2.5 align-top">
                    {grant.constraints &&
                    Object.keys(grant.constraints).length > 0 ? (
                      <ConstraintChips c={grant.constraints} />
                    ) : (
                      <span className="text-xs text-gray-300">—</span>
                    )}
                  </td>
                )}

                {hasSources && (
                  <td className="px-4 py-2.5 align-top">
                    {renderSource?.(grant, index)}
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
