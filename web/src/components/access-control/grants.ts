import type { ComponentType } from "react";
import { Bot, ListTodo, MessageSquare, User } from "lucide-react";
import type { RbacResource } from "@/api/models/RbacResource";
import { navIcons } from "@/components/layout/navIcons";

export interface GrantConstraints {
  managing_pack_id?: number;
  pack_refs?: string[];
  owner?: string;
  owner_types?: string[];
  owner_refs?: string[];
  visibility?: string[];
  execution_scope?: string;
  refs?: string[];
  encrypted?: boolean;
  attributes?: Record<string, unknown>;
}

export interface ParsedGrant {
  resource: string;
  actions: string[];
  scope?: "pack" | "external";
  constraints?: GrantConstraints;
}

type ResourceMeta = {
  icon: ComponentType<{ className?: string }>;
  color: string;
  label: string;
};

export const RESOURCE_META: Record<string, ResourceMeta> = {
  packs: { icon: navIcons.packs, color: "text-green-600", label: "Packs" },
  actions: {
    icon: navIcons.actions,
    color: "text-yellow-500",
    label: "Actions",
  },
  policies: {
    icon: navIcons.policies,
    color: "text-amber-600",
    label: "Policies",
  },
  rules: { icon: navIcons.rules, color: "text-blue-600", label: "Rules" },
  triggers: {
    icon: navIcons.triggers,
    color: "text-orange-500",
    label: "Triggers",
  },
  executions: {
    icon: navIcons.traces,
    color: "text-purple-600",
    label: "Executions / Traces",
  },
  events: { icon: navIcons.events, color: "text-cyan-600", label: "Events" },
  enforcements: {
    icon: navIcons.enforcements,
    color: "text-red-500",
    label: "Enforcements",
  },
  inquiries: {
    icon: MessageSquare,
    color: "text-teal-600",
    label: "Inquiries",
  },
  keys: { icon: navIcons.keys, color: "text-amber-600", label: "Keys" },
  caches: {
    icon: navIcons.caches,
    color: "text-teal-700",
    label: "Data Caches",
  },
  artifacts: {
    icon: navIcons.artifacts,
    color: "text-indigo-500",
    label: "Artifacts",
  },
  identities: { icon: User, color: "text-blue-700", label: "Identities" },
  permissions: {
    icon: navIcons.accessControl,
    color: "text-indigo-600",
    label: "Permissions",
  },
  runtimes: {
    icon: navIcons.runtimes,
    color: "text-blue-600",
    label: "Runtimes",
  },
  workers: {
    icon: Bot,
    color: "text-blue-700",
    label: "Workers",
  },
  queues: {
    icon: navIcons.queues,
    color: "text-emerald-600",
    label: "Work Queues",
  },
  queue_items: {
    icon: ListTodo,
    color: "text-emerald-600",
    label: "Queue Items",
  },
  dashboards: {
    icon: navIcons.dashboard,
    color: "text-rose-500",
    label: "Dashboards",
  },
  retention: {
    icon: navIcons.retention,
    color: "text-teal-700",
    label: "Data Retention",
  },
  audit_log: {
    icon: navIcons.auditLog,
    color: "text-slate-600",
    label: "Audit Log",
  },
} satisfies Record<RbacResource, ResourceMeta>;

export const ACTION_STYLE: Record<string, string> = {
  read: "bg-slate-100 text-slate-700",
  create: "bg-emerald-100 text-emerald-800",
  install: "bg-blue-100 text-blue-800",
  configure: "bg-amber-100 text-amber-800",
  update: "bg-amber-100 text-amber-800",
  delete: "bg-red-100 text-red-800",
  execute: "bg-violet-100 text-violet-800",
  cancel: "bg-orange-100 text-orange-800",
  respond: "bg-cyan-100 text-cyan-800",
  manage: "bg-indigo-100 text-indigo-800",
  decrypt: "bg-pink-100 text-pink-800",
};

export function parseGrants(raw: unknown): ParsedGrant[] {
  if (!Array.isArray(raw)) return [];
  return raw.filter(
    (grant): grant is ParsedGrant =>
      typeof grant === "object" &&
      grant !== null &&
      "resource" in grant &&
      typeof grant.resource === "string" &&
      "actions" in grant &&
      Array.isArray(grant.actions) &&
      grant.actions.every((action: unknown) => typeof action === "string") &&
      (!("scope" in grant) ||
        grant.scope === "pack" ||
        grant.scope === "external"),
  );
}

// Normalized IAM grants contain server-owned IDs, not editable authored scope.
export function permissionSetAuthoring(raw: unknown):
  | {
      scope: "pack" | "external";
      grants: ParsedGrant[];
    }
  | undefined {
  if (
    typeof raw !== "object" ||
    raw === null ||
    !("authored_definition" in raw)
  )
    return;
  const definition = raw.authored_definition;
  if (
    typeof definition !== "object" ||
    definition === null ||
    !("scope" in definition) ||
    (definition.scope !== "pack" && definition.scope !== "external") ||
    !("grants" in definition) ||
    !Array.isArray(definition.grants)
  )
    return;
  const grants = parseGrants(definition.grants);
  if (grants.length !== definition.grants.length) return;
  return { scope: definition.scope, grants };
}
