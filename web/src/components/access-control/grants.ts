import type { ComponentType } from "react";
import {
  BarChart3,
  Bot,
  Globe,
  History,
  MessageSquare,
  User,
} from "lucide-react";
import { navIcons } from "@/components/layout/navIcons";

export interface GrantConstraints {
  pack_refs?: string[];
  owner?: string;
  owner_types?: string[];
  visibility?: string[];
  execution_scope?: string;
  refs?: string[];
  encrypted?: boolean;
  attributes?: Record<string, unknown>;
}

export interface ParsedGrant {
  resource: string;
  actions: string[];
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
  artifacts: {
    icon: navIcons.artifacts,
    color: "text-indigo-500",
    label: "Artifacts",
  },
  webhooks: { icon: Globe, color: "text-sky-600", label: "Webhooks" },
  analytics: { icon: BarChart3, color: "text-rose-500", label: "Analytics" },
  history: { icon: History, color: "text-gray-500", label: "History" },
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
  sensors: {
    icon: navIcons.sensors,
    color: "text-purple-600",
    label: "Sensors",
  },
  queues: {
    icon: navIcons.queues,
    color: "text-emerald-600",
    label: "Queues",
  },
  audit_log: {
    icon: navIcons.auditLog,
    color: "text-slate-600",
    label: "Audit Log",
  },
};

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
      typeof (grant as ParsedGrant).resource === "string" &&
      Array.isArray((grant as ParsedGrant).actions),
  );
}
