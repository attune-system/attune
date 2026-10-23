import { describe, expect, it } from "vitest";
import { approvalPurpose } from "./approval-purpose";

const inputTarget = {
  policy_revision: 17,
  invocation_binding: {
    kind: "workflow_task",
    workflow_ref: "example.flow",
    task_path: "/tasks/~1named",
    action_ref: "example.reader",
  },
  action_id: 33,
  action_ref: "example.reader",
  pack_id: 7,
  pack_ref: "example",
  release_id: 44,
  metadata_revision: 55,
  declaration_revision: "d".repeat(64),
  source_path: "/input/token",
  expression: "{{ keystore['shared.token'] }}",
};

describe("approval purpose wire metadata", () => {
  it("accepts only explicit runtime purpose, never missing or ambiguous metadata", () => {
    expect(approvalPurpose({ kind: "runtime" })).toEqual({ kind: "runtime" });
    for (const value of [
      undefined,
      null,
      [],
      {},
      "runtime",
      { kind: "unknown" },
      { kind: "runtime", targets: [] },
      { kind: "runtime", extra: true },
    ]) {
      expect(approvalPurpose(value)).toEqual({ kind: "unavailable" });
    }
  });

  it("retains exact immutable visible targets without treating discovery metadata as runtime use", () => {
    expect(
      approvalPurpose({ kind: "declared_input", targets: [inputTarget] }),
    ).toEqual({
      kind: "declared_input",
      targets: [inputTarget],
      metadataAvailable: true,
    });
    expect(approvalPurpose({ kind: "declared_input", targets: [] })).toEqual({
      kind: "declared_input",
      targets: [],
      metadataAvailable: true,
    });
  });

  it("validates all four actual binding variants despite the generated discriminator defect", () => {
    for (const invocation_binding of [
      { kind: "action", action_ref: "example.reader" },
      inputTarget.invocation_binding,
      { kind: "rule", action_ref: "example.reader", rule_ref: "example.route" },
      {
        kind: "queue",
        action_ref: "example.reader",
        queue_ref: "example.jobs",
      },
    ]) {
      const target = {
        ...inputTarget,
        invocation_binding,
        metadata_revision: null,
      };
      expect(
        approvalPurpose({ kind: "declared_input", targets: [target] }),
      ).toEqual({
        kind: "declared_input",
        targets: [target],
        metadataAvailable: true,
      });
    }
  });

  it("does not silently substitute runtime when input targets are unavailable, invalid or overbound", () => {
    for (const value of [
      { kind: "declared_input" },
      { kind: "declared_input", targets: null },
      { kind: "declared_input", targets: {} },
      { kind: "declared_input", targets: [null] },
      { kind: "declared_input", targets: [inputTarget], extra: true },
      {
        kind: "declared_input",
        targets: Array.from({ length: 257 }, () => inputTarget),
      },
    ])
      expect(approvalPurpose(value)).toEqual({
        kind: "declared_input",
        targets: [],
        metadataAvailable: false,
      });
  });

  it("never guesses rounded IDs or renders unexpected source/configuration data", () => {
    for (const target of [
      { ...inputTarget, policy_revision: 0 },
      { ...inputTarget, release_id: "44" },
      { ...inputTarget, action_id: Number.MAX_SAFE_INTEGER + 1 },
      { ...inputTarget, metadata_revision: -1 },
      { ...inputTarget, source_path: null },
      { ...inputTarget, expression: {} },
      { ...inputTarget, evaluated_value: "private-value" },
      {
        ...inputTarget,
        invocation_binding: {
          kind: "workflow_task",
          action_ref: "example.reader",
        },
      },
      {
        ...inputTarget,
        invocation_binding: { kind: "future", action_ref: "example.reader" },
      },
      {
        ...inputTarget,
        invocation_binding: {
          kind: "action",
          action_ref: "example.reader",
          config: "private-value",
        },
      },
    ]) {
      const result = approvalPurpose({
        kind: "declared_input",
        targets: [target],
      });
      expect(result).toEqual({
        kind: "declared_input",
        targets: [],
        metadataAvailable: false,
      });
      expect(JSON.stringify(result)).not.toContain("private-value");
    }
  });
});
