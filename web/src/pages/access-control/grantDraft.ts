import type {
  GrantConstraints,
  ParsedGrant,
} from "@/components/access-control/grants";

// Pure grant <-> draft-form conversion logic for the permission-set editor,
// kept in its own module (rather than inline in PermissionSetDetailPage.tsx)
// so it can be unit tested directly and so the page file only exports its
// default component (required by the `react-refresh/only-export-components`
// lint rule).

export const ALL_ACTIONS = [
  "read",
  "create",
  "install",
  "configure",
  "update",
  "delete",
  "execute",
  "cancel",
  "respond",
  "manage",
  "decrypt",
];

export const RESOURCE_ACTIONS: Record<string, string[]> = {
  packs: ["read", "create", "install", "configure", "delete"],
  actions: ["read", "create", "update", "delete", "execute"],
  queues: ["read", "create", "update", "delete"],
  rules: ["read", "create", "update", "delete"],
  triggers: ["read", "create", "update", "delete"],
  executions: ["read", "update", "cancel"],
  events: ["read"],
  enforcements: ["read"],
  inquiries: ["read", "create", "update", "delete", "respond"],
  keys: ["read", "create", "update", "delete", "decrypt"],
  caches: ["read", "create", "update", "delete"],
  artifacts: ["read", "create", "update", "delete"],
  runtimes: ["read", "create", "update", "delete"],
  workers: ["read"],
  identities: ["read", "create", "update", "delete"],
  permissions: ["read", "manage"],
  audit_log: ["read"],
};

export const PACK_SCOPED_RESOURCES = new Set([
  "packs",
  "actions",
  "queues",
  "rules",
  "triggers",
  "artifacts",
]);
export const COMPONENT_SCOPED_RESOURCES = new Set([
  "packs",
  "actions",
  "queues",
  "rules",
  "triggers",
  "executions",
  "keys",
  "artifacts",
  "caches",
]);
export const OWNER_SCOPED_RESOURCES = new Set(["packs", "keys", "artifacts"]);
export const OWNER_TYPE_RESOURCES = new Set(["keys", "artifacts", "caches"]);
// Cache grants scope to a specific owner *reference* (e.g. a pack ref) in
// addition to the owner *type*, combined with a namespace `refs` constraint —
// see the `cache_grant_matches_owner_type_owner_ref_and_namespace_ref` test in
// crates/common/src/rbac.rs. Owner-ref scoping isn't meaningful for the
// self/any/none-style OWNER_SCOPED_RESOURCES above, so it's kept distinct.
export const OWNER_REF_RESOURCES = new Set(["caches"]);

// Cache namespaces are the "component" being scoped, so this resource gets
// cache-specific copy instead of the generic "Component scoped" wording.
export function componentScopeOptionLabel(resource: string): string {
  return resource === "caches" ? "Namespace scoped" : "Component scoped";
}

export function componentScopeFieldLabel(resource: string): string {
  return resource === "caches" ? "Namespace refs" : "Component refs";
}

export function componentScopePlaceholder(resource: string): string {
  return resource === "caches"
    ? "users, locations"
    : "core.echo, slack.post_message";
}

export type ScopeType = "unconstrained" | "pack" | "component";

export type GrantDraft = {
  id: string;
  resource: string;
  actions: string[];
  scopeType: ScopeType;
  scopeRefs: string;
  owner: string;
  ownerTypes: string;
  ownerRefs: string;
  visibility: string[];
  executionScope: string;
  encrypted: string;
  attributes: string;
};

export function csv(values: string[] | undefined): string {
  return values?.join(", ") ?? "";
}

export function splitCsv(value: string): string[] | undefined {
  const values = value
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
  return values.length > 0 ? values : undefined;
}

export function grantToDraft(grant: ParsedGrant, index: number): GrantDraft {
  const constraints = grant.constraints ?? {};
  const scopeType: ScopeType = constraints.pack_refs?.length
    ? "pack"
    : constraints.refs?.length
      ? "component"
      : "unconstrained";
  return {
    id: `${index}-${grant.resource}`,
    resource: grant.resource,
    actions: grant.actions.filter((action) =>
      (RESOURCE_ACTIONS[grant.resource] ?? ALL_ACTIONS).includes(action),
    ),
    scopeType,
    scopeRefs:
      scopeType === "pack"
        ? csv(constraints.pack_refs)
        : scopeType === "component"
          ? csv(constraints.refs)
          : "",
    owner: constraints.owner ?? "",
    ownerTypes: csv(constraints.owner_types),
    ownerRefs: csv(constraints.owner_refs),
    visibility: constraints.visibility ?? [],
    executionScope: constraints.execution_scope ?? "",
    encrypted:
      constraints.encrypted === undefined
        ? ""
        : constraints.encrypted
          ? "true"
          : "false",
    attributes: constraints.attributes
      ? JSON.stringify(constraints.attributes, null, 2)
      : "",
  };
}

export function draftToGrant(draft: GrantDraft): ParsedGrant {
  const validActions = RESOURCE_ACTIONS[draft.resource] ?? [];
  const actions = draft.actions.filter((action) =>
    validActions.includes(action),
  );
  if (draft.actions.length === 0) {
    throw new Error("Each grant must include at least one permission spec.");
  }
  if (actions.length === 0) {
    throw new Error(`No selected permission specs apply to ${draft.resource}.`);
  }

  const constraints: GrantConstraints = {};
  const ownerTypes = splitCsv(draft.ownerTypes);
  const ownerRefs = splitCsv(draft.ownerRefs);
  const scopeRefs = splitCsv(draft.scopeRefs);

  if (draft.scopeType === "pack") {
    if (!PACK_SCOPED_RESOURCES.has(draft.resource)) {
      throw new Error(`${draft.resource} grants cannot be pack scoped.`);
    }
    if (!scopeRefs) {
      throw new Error("Pack-scoped grants require at least one pack ref.");
    }
    constraints.pack_refs = scopeRefs;
  } else if (draft.scopeType === "component") {
    if (!COMPONENT_SCOPED_RESOURCES.has(draft.resource)) {
      throw new Error(`${draft.resource} grants cannot be component scoped.`);
    }
    if (!scopeRefs) {
      throw new Error(
        "Component-scoped grants require at least one component ref.",
      );
    }
    constraints.refs = scopeRefs;
  }

  if (draft.owner && OWNER_SCOPED_RESOURCES.has(draft.resource)) {
    constraints.owner = draft.owner;
  }
  if (ownerTypes && OWNER_TYPE_RESOURCES.has(draft.resource)) {
    constraints.owner_types = ownerTypes;
  }
  if (ownerRefs && OWNER_REF_RESOURCES.has(draft.resource)) {
    constraints.owner_refs = ownerRefs;
  }
  if (draft.visibility.length > 0 && draft.resource === "artifacts") {
    constraints.visibility = draft.visibility;
  }
  if (draft.executionScope && draft.resource === "executions") {
    constraints.execution_scope = draft.executionScope;
  }
  if (draft.encrypted && draft.resource === "keys") {
    constraints.encrypted = draft.encrypted === "true";
  }
  if (draft.attributes.trim()) {
    const parsed = JSON.parse(draft.attributes);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("Attribute constraints must be a JSON object.");
    }
    constraints.attributes = parsed as Record<string, unknown>;
  }

  return {
    resource: draft.resource,
    actions: [...actions].sort(),
    ...(Object.keys(constraints).length > 0 ? { constraints } : {}),
  };
}

export function newGrantDraft(): GrantDraft {
  return {
    id: crypto.randomUUID(),
    resource: "actions",
    actions: ["read"],
    scopeType: "unconstrained",
    scopeRefs: "",
    owner: "",
    ownerTypes: "",
    ownerRefs: "",
    visibility: [],
    executionScope: "",
    encrypted: "",
    attributes: "",
  };
}

export function normalizeDraft(draft: GrantDraft): GrantDraft {
  const validActions = RESOURCE_ACTIONS[draft.resource] ?? [];
  const actions = draft.actions.filter((action) =>
    validActions.includes(action),
  );
  const scopeType =
    draft.scopeType === "pack" && !PACK_SCOPED_RESOURCES.has(draft.resource)
      ? "unconstrained"
      : draft.scopeType === "component" &&
          !COMPONENT_SCOPED_RESOURCES.has(draft.resource)
        ? "unconstrained"
        : draft.scopeType;

  return {
    ...draft,
    actions: actions.length > 0 ? actions : validActions.slice(0, 1),
    scopeType,
    scopeRefs: scopeType === "unconstrained" ? "" : draft.scopeRefs,
    owner: OWNER_SCOPED_RESOURCES.has(draft.resource) ? draft.owner : "",
    ownerTypes: OWNER_TYPE_RESOURCES.has(draft.resource)
      ? draft.ownerTypes
      : "",
    ownerRefs: OWNER_REF_RESOURCES.has(draft.resource) ? draft.ownerRefs : "",
    visibility: draft.resource === "artifacts" ? draft.visibility : [],
    executionScope: draft.resource === "executions" ? draft.executionScope : "",
    encrypted: draft.resource === "keys" ? draft.encrypted : "",
  };
}
