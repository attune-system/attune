import type { DeclaredInputApprovalTarget } from "@/api/models/DeclaredInputApprovalTarget";

type VisibleBinding = DeclaredInputApprovalTarget["invocation_binding"];
type VisibleTarget = DeclaredInputApprovalTarget;

export type ApprovalPurposeView =
  | { kind: "runtime" }
  | {
      kind: "declared_input";
      targets: VisibleTarget[];
      metadataAvailable: boolean;
    }
  | { kind: "unavailable" };

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function object(value: unknown, fields: readonly string[]) {
  if (
    !isObject(value) ||
    Object.keys(value).some((key) => !fields.includes(key))
  ) {
    throw new Error("Invalid approval purpose metadata.");
  }
  return value;
}

function text(value: unknown): string {
  if (typeof value !== "string") throw new Error("Invalid metadata string.");
  return value;
}

function id(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value <= 0) {
    throw new Error("Metadata ID cannot be represented exactly.");
  }
  return value;
}

function binding(value: unknown): VisibleBinding {
  if (!isObject(value)) throw new Error("Missing invocation binding.");
  const action_ref = text(value.action_ref);
  switch (value.kind) {
    case "action":
      object(value, ["kind", "action_ref"]);
      return { kind: "action", action_ref };
    case "workflow_task":
      object(value, ["kind", "action_ref", "workflow_ref", "task_path"]);
      return {
        kind: "workflow_task",
        action_ref,
        workflow_ref: text(value.workflow_ref),
        task_path: text(value.task_path),
      };
    case "rule":
      object(value, ["kind", "action_ref", "rule_ref"]);
      return { kind: "rule", action_ref, rule_ref: text(value.rule_ref) };
    case "queue":
      object(value, ["kind", "action_ref", "queue_ref"]);
      return { kind: "queue", action_ref, queue_ref: text(value.queue_ref) };
    default:
      throw new Error("Unknown invocation binding.");
  }
}

function target(value: unknown): VisibleTarget {
  const input = object(value, [
    "action_id",
    "action_ref",
    "declaration_revision",
    "expression",
    "invocation_binding",
    "metadata_revision",
    "pack_id",
    "pack_ref",
    "policy_revision",
    "release_id",
    "source_path",
  ]);
  const result: VisibleTarget = {
    action_id: id(input.action_id),
    action_ref: text(input.action_ref),
    declaration_revision: text(input.declaration_revision),
    expression: text(input.expression),
    invocation_binding: binding(input.invocation_binding),
    pack_id: id(input.pack_id),
    pack_ref: text(input.pack_ref),
    policy_revision: id(input.policy_revision),
    release_id: id(input.release_id),
    source_path: text(input.source_path),
  };
  if ("metadata_revision" in input)
    result.metadata_revision =
      input.metadata_revision === null ? null : id(input.metadata_revision);
  return result;
}

export function approvalPurpose(value: unknown): ApprovalPurposeView {
  if (!isObject(value)) return { kind: "unavailable" };
  if (
    value.kind === "runtime" &&
    Object.hasOwn(value, "kind") &&
    Object.keys(value).every((key) => key === "kind")
  )
    return { kind: "runtime" };
  if (value.kind !== "declared_input") return { kind: "unavailable" };
  try {
    object(value, ["kind", "targets"]);
    if (!Array.isArray(value.targets) || value.targets.length > 256)
      throw new Error("Invalid target list.");
    const targets: unknown[] = value.targets;
    return {
      kind: "declared_input",
      targets: targets.map(target),
      metadataAvailable: true,
    };
  } catch {
    // Missing/collected/malformed target metadata must never confer runtime use.
    return { kind: "declared_input", targets: [], metadataAvailable: false };
  }
}

export function approvalPurposeLabel(
  kind: ApprovalPurposeView["kind"],
): string {
  switch (kind) {
    case "runtime":
      return "Runtime API authority";
    case "declared_input":
      return "Input-only consent";
    case "unavailable":
      return "Purpose unavailable";
  }
}
