import type { CurrentUserResponse } from "@/api";

export const STANDARD_EXECUTION_ACCESS_REF = "standard";

export type PermissionRequirement = {
  resource: string;
  actions?: string[];
};

export function hasPermission(
  user: CurrentUserResponse | null,
  resource: string,
  action = "read",
): boolean {
  return (
    user?.effective_permissions?.some(
      (permission) =>
        permission.resource === resource && permission.actions.includes(action),
    ) ?? false
  );
}

export function hasAnyPermission(
  user: CurrentUserResponse | null,
  requirements: PermissionRequirement[] | undefined,
): boolean {
  if (!requirements || requirements.length === 0) {
    return true;
  }

  return requirements.some((requirement) =>
    (requirement.actions ?? ["read"]).some((action) =>
      hasPermission(user, requirement.resource, action),
    ),
  );
}

export function requirementsForPath(
  pathname: string,
): PermissionRequirement[] | null {
  if (pathname === "/" || pathname.startsWith("/profile")) {
    return null;
  }

  const rules: Array<{
    prefix: string;
    requirements: PermissionRequirement[];
  }> = [
    { prefix: "/actions", requirements: [{ resource: "actions" }] },
    { prefix: "/rules", requirements: [{ resource: "rules" }] },
    { prefix: "/queues", requirements: [{ resource: "queues" }] },
    { prefix: "/policies", requirements: [{ resource: "policies" }] },
    { prefix: "/triggers", requirements: [{ resource: "triggers" }] },
    { prefix: "/sensors", requirements: [{ resource: "triggers" }] },
    { prefix: "/executions", requirements: [{ resource: "executions" }] },
    { prefix: "/enforcements", requirements: [{ resource: "enforcements" }] },
    { prefix: "/events", requirements: [{ resource: "events" }] },
    { prefix: "/traces", requirements: [{ resource: "executions" }] },
    { prefix: "/artifacts", requirements: [{ resource: "artifacts" }] },
    { prefix: "/keys", requirements: [{ resource: "keys" }] },
    {
      prefix: "/access-control",
      requirements: [{ resource: "identities" }, { resource: "permissions" }],
    },
    { prefix: "/audit-log", requirements: [{ resource: "audit_log" }] },
    { prefix: "/retention", requirements: [{ resource: "retention" }] },
    { prefix: "/packs", requirements: [{ resource: "packs" }] },
    {
      prefix: "/runtimes",
      requirements: [{ resource: "runtimes" }, { resource: "workers" }],
    },
  ];

  return (
    rules.find((rule) => pathname.startsWith(rule.prefix))?.requirements ?? null
  );
}
