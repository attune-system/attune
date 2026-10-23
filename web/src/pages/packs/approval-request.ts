import type { AuthoredGrantConstraints } from "@/api/models/AuthoredGrantConstraints";
import type { IssueRuntimeApprovalRequest } from "@/api/models/IssueRuntimeApprovalRequest";
import type { PermissionGrantDefinition } from "@/api/models/PermissionGrantDefinition";
import { ArtifactVisibility } from "@/api/models/ArtifactVisibility";
import { OwnerType } from "@/api/models/OwnerType";
import { PermissionScope } from "@/api/models/PermissionScope";
import { RbacAction } from "@/api/models/RbacAction";
import { RbacExecutionScopeConstraint } from "@/api/models/RbacExecutionScopeConstraint";
import { RbacOwnerConstraint } from "@/api/models/RbacOwnerConstraint";
import { RbacResource } from "@/api/models/RbacResource";

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function object(value: unknown, fields: readonly string[], label: string) {
  if (
    !isObject(value) ||
    Object.keys(value).some((key) => !fields.includes(key))
  ) {
    throw new Error(`${label} contains unknown fields or is not an object.`);
  }
  return value;
}

function enumValue<T extends string>(
  value: unknown,
  values: readonly T[],
  label: string,
): T {
  for (const member of values) {
    if (member === value) return member;
  }
  throw new Error(`${label} is not a supported value.`);
}

function array<T>(
  value: unknown,
  parse: (item: unknown) => T,
  label: string,
): T[] {
  if (!Array.isArray(value) || value.length === 0) {
    throw new Error(`${label} must be a nonempty array.`);
  }
  const items: unknown[] = value;
  return items.map(parse);
}

function reference(value: unknown): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error("Scope references must be nonempty strings.");
  }
  return value;
}

function constraints(value: unknown): AuthoredGrantConstraints | null {
  if (value === null) return null;
  const input = object(
    value,
    [
      "attributes",
      "encrypted",
      "execution_scope",
      "owner",
      "owner_refs",
      "owner_types",
      "pack_refs",
      "refs",
      "visibility",
    ],
    "Grant constraints",
  );
  const result: AuthoredGrantConstraints = {};
  if ("attributes" in input) {
    // Fixed runtime approvals cannot depend on a human's mutable attributes.
    if (input.attributes === null) result.attributes = null;
    else if (
      isObject(input.attributes) &&
      Object.keys(input.attributes).length === 0
    )
      result.attributes = {};
    else
      throw new Error(
        "Fixed approvals cannot use identity attribute conditions.",
      );
  }
  if ("encrypted" in input) {
    if (input.encrypted !== null && typeof input.encrypted !== "boolean") {
      throw new Error("encrypted must be a boolean or null.");
    }
    result.encrypted = input.encrypted;
  }
  if ("owner" in input) {
    const owner =
      input.owner === null
        ? null
        : enumValue(input.owner, Object.values(RbacOwnerConstraint), "owner");
    if (owner === RbacOwnerConstraint.SELF)
      throw new Error("Fixed approvals cannot use a human-relative owner.");
    result.owner = owner;
  }
  if ("execution_scope" in input) {
    const scope =
      input.execution_scope === null
        ? null
        : enumValue(
            input.execution_scope,
            Object.values(RbacExecutionScopeConstraint),
            "execution_scope",
          );
    if (scope !== null && scope !== RbacExecutionScopeConstraint.ANY) {
      throw new Error(
        "Fixed approvals cannot use a human-relative execution scope.",
      );
    }
    result.execution_scope = scope;
  }
  for (const field of [
    "refs",
    "pack_refs",
    "owner_refs",
  ] satisfies (keyof AuthoredGrantConstraints)[]) {
    if (field in input)
      result[field] =
        input[field] === null ? null : array(input[field], reference, field);
  }
  if ("owner_types" in input) {
    result.owner_types =
      input.owner_types === null
        ? null
        : array(
            input.owner_types,
            (item) => enumValue(item, Object.values(OwnerType), "owner_types"),
            "owner_types",
          );
  }
  if ("visibility" in input) {
    result.visibility =
      input.visibility === null
        ? null
        : array(
            input.visibility,
            (item) =>
              enumValue(item, Object.values(ArtifactVisibility), "visibility"),
            "visibility",
          );
  }
  return result;
}

function grant(value: unknown): PermissionGrantDefinition {
  const input = object(
    value,
    ["resource", "actions", "constraints", "scope"],
    "Grant",
  );
  const result: PermissionGrantDefinition = {
    resource: enumValue(
      input.resource,
      Object.values(RbacResource),
      "resource",
    ),
    actions: array(
      input.actions,
      (item) => enumValue(item, Object.values(RbacAction), "actions"),
      "actions",
    ),
  };
  if ("scope" in input) {
    const scope =
      input.scope === null
        ? null
        : enumValue(input.scope, Object.values(PermissionScope), "scope");
    if (scope === PermissionScope.PACK)
      throw new Error(
        "External approvals cannot contain pack-local scope declarations.",
      );
    result.scope = scope;
  }
  if ("constraints" in input)
    result.constraints = constraints(input.constraints);
  return result;
}

export function parseApprovalRequest(
  text: string,
): IssueRuntimeApprovalRequest {
  const parsed: unknown = JSON.parse(text);
  const input = object(parsed, ["grants", "expires_at"], "Approval request");
  const grants = array(input.grants, grant, "grants");
  if (grants.length > 256) throw new Error("Provide 1 to 256 grant objects.");
  const expiresAt = input.expires_at;
  if (expiresAt === undefined || expiresAt === null)
    return { grants, expires_at: null };
  if (
    typeof expiresAt !== "string" ||
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(
      expiresAt,
    ) ||
    !Number.isFinite(Date.parse(expiresAt)) ||
    Date.parse(expiresAt) <= Date.now()
  )
    throw new Error("expires_at must be a future RFC3339 timestamp or null.");
  return { grants, expires_at: expiresAt };
}

export function parseInputConsentRequest(
  text: string,
): IssueRuntimeApprovalRequest {
  const request = parseApprovalRequest(text);
  if (
    request.grants.some(
      (grant) =>
        (grant.resource !== RbacResource.KEYS &&
          grant.resource !== RbacResource.PACKS) ||
        grant.actions.some(
          (action) =>
            action !== RbacAction.READ && action !== RbacAction.DECRYPT,
        ),
    )
  )
    throw new Error(
      "Input-only consent accepts only keys/packs read or decrypt grants, never execution API permissions.",
    );
  return request;
}
