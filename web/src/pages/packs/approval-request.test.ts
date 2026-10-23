import { describe, expect, it } from "vitest";
import { ArtifactVisibility } from "@/api/models/ArtifactVisibility";
import { OwnerType } from "@/api/models/OwnerType";
import { PermissionScope } from "@/api/models/PermissionScope";
import { RbacAction } from "@/api/models/RbacAction";
import { RbacExecutionScopeConstraint } from "@/api/models/RbacExecutionScopeConstraint";
import { RbacOwnerConstraint } from "@/api/models/RbacOwnerConstraint";
import { RbacResource } from "@/api/models/RbacResource";
import {
  parseApprovalRequest,
  parseInputConsentRequest,
} from "./approval-request";

const grant = { resource: RbacResource.KEYS, actions: [RbacAction.READ] };
const parse = (value: unknown) => parseApprovalRequest(JSON.stringify(value));

describe("canonical fixed approval request boundary", () => {
  it("input consent uses the server's read-only keys/packs ceiling without altering runtime grants", () => {
    for (const resource of [RbacResource.KEYS, RbacResource.PACKS]) {
      expect(
        parseInputConsentRequest(
          JSON.stringify({
            grants: [
              { resource, actions: [RbacAction.READ, RbacAction.DECRYPT] },
            ],
          }),
        ).grants,
      ).toEqual([{ resource, actions: [RbacAction.READ, RbacAction.DECRYPT] }]);
    }
    for (const resource of Object.values(RbacResource)) {
      if (resource !== RbacResource.KEYS && resource !== RbacResource.PACKS) {
        expect(() =>
          parseInputConsentRequest(
            JSON.stringify({
              grants: [{ resource, actions: [RbacAction.READ] }],
            }),
          ),
        ).toThrow(/Input-only/);
      }
    }
    for (const action of Object.values(RbacAction)) {
      if (action !== RbacAction.READ && action !== RbacAction.DECRYPT) {
        expect(() =>
          parseInputConsentRequest(
            JSON.stringify({
              grants: [{ resource: RbacResource.KEYS, actions: [action] }],
            }),
          ),
        ).toThrow(/Input-only/);
      }
    }
    expect(
      parseApprovalRequest(
        JSON.stringify({
          grants: [
            { resource: RbacResource.EXECUTIONS, actions: [RbacAction.CANCEL] },
          ],
        }),
      ).grants[0].actions,
    ).toEqual([RbacAction.CANCEL]);
  });
  it("constructs generated grants and preserves every supported authored constraint", () => {
    const constraints = {
      refs: ["shared.token"],
      pack_refs: ["shared"],
      owner_refs: ["shared.reader"],
      owner: RbacOwnerConstraint.ANY,
      execution_scope: RbacExecutionScopeConstraint.ANY,
      owner_types: [OwnerType.PACK, OwnerType.ACTION],
      visibility: [ArtifactVisibility.PRIVATE],
      encrypted: true,
      attributes: {},
    };
    const input = {
      grants: [{ ...grant, scope: PermissionScope.EXTERNAL, constraints }],
      expires_at: "2099-12-01T02:03:04+02:00",
    };
    expect(parse(input)).toEqual(input);
  });

  it("preserves omission and canonical nullable constraint/scope fields", () => {
    const constraints = {
      refs: null,
      pack_refs: null,
      owner_refs: null,
      owner: null,
      execution_scope: null,
      owner_types: null,
      visibility: null,
      encrypted: null,
      attributes: null,
    };
    expect(parse({ grants: [{ ...grant, scope: null, constraints }] })).toEqual(
      {
        grants: [{ ...grant, scope: null, constraints }],
        expires_at: null,
      },
    );
    expect(parse({ grants: [{ ...grant, constraints: null }] })).toEqual({
      grants: [{ ...grant, constraints: null }],
      expires_at: null,
    });
    expect(parse({ grants: [grant] })).toEqual({
      grants: [grant],
      expires_at: null,
    });
  });

  it("retains supported enum values without inventing an action/resource compatibility matrix", () => {
    for (const resource of Object.values(RbacResource)) {
      expect(
        parse({ grants: [{ resource, actions: [RbacAction.READ] }] }).grants[0]
          .resource,
      ).toBe(resource);
    }
    for (const action of Object.values(RbacAction)) {
      expect(
        parse({ grants: [{ ...grant, actions: [action] }] }).grants[0].actions,
      ).toEqual([action]);
    }
    for (const owner of [RbacOwnerConstraint.ANY, RbacOwnerConstraint.NONE]) {
      expect(
        parse({ grants: [{ ...grant, constraints: { owner } }] }).grants[0]
          .constraints?.owner,
      ).toBe(owner);
    }
  });

  it("bounds grants without silently truncating them", () => {
    expect(
      parse({ grants: Array.from({ length: 256 }, () => grant) }).grants,
    ).toHaveLength(256);
    expect(() =>
      parse({ grants: Array.from({ length: 257 }, () => grant) }),
    ).toThrow();
  });

  it("rejects unknown actions/resources and malformed grant shapes", () => {
    for (const value of [
      null,
      [],
      {},
      { grants: [] },
      { grants: [null] },
      { grants: [[]] },
      { grants: [{ ...grant, resource: "unknown" }] },
      { grants: [{ ...grant, resource: "KEYS" }] },
      { grants: [{ ...grant, resource: "*" }] },
      { grants: [{ ...grant, actions: [] }] },
      { grants: [{ ...grant, actions: "read" }] },
      { grants: [{ ...grant, actions: ["read", "unknown"] }] },
      { grants: [{ ...grant, actions: ["read", 42] }] },
      { grants: [{ ...grant, actions: ["READ"] }] },
      { grants: [{ ...grant, actions: ["*"] }] },
      { grants: [{ ...grant, constraints: [] }] },
      { grants: [{ ...grant, constraints: false }] },
      { grants: [{ ...grant, scope: "pack" }] },
      { grants: [{ ...grant, scope: "unknown" }] },
      { grants: [{ ...grant, scope: {} }] },
    ])
      expect(() => parse(value)).toThrow();
  });

  it("rejects server-owned bindings and extra fields at every authoring boundary", () => {
    for (const field of [
      "ids",
      "managing_pack_id",
      "owner_ids",
      "targets",
      "concrete_targets",
      "grant_digest",
      "unexpected",
    ]) {
      expect(() => parse({ grants: [grant], [field]: 1 })).toThrow();
      expect(() => parse({ grants: [{ ...grant, [field]: 1 }] })).toThrow();
      expect(() =>
        parse({ grants: [{ ...grant, constraints: { [field]: 1 } }] }),
      ).toThrow();
    }
    for (const field of [
      "purpose",
      "renderer_execution",
      "policy_revision",
      "invocation_binding",
      "source_path",
      "expression",
      "declaration_revision",
    ]) {
      expect(() =>
        parse({ grants: [grant], [field]: "not-runtime" }),
      ).toThrow();
    }
    expect(() =>
      parseApprovalRequest(
        '{"grants":[{"resource":"keys","actions":["read"],"__proto__":{"scope":"external"}}]}',
      ),
    ).toThrow();
    expect(() =>
      parseApprovalRequest(
        '{"grants":[{"resource":"keys","actions":["read"],"constraints":{"__proto__":{"ids":[1]}}}]}',
      ),
    ).toThrow();
  });

  it("checks exact nested enums, nonempty arrays and booleans rather than trusting objects", () => {
    const invalid = [
      { refs: "shared.token" },
      { refs: [] },
      { refs: [42] },
      { refs: [" "] },
      { pack_refs: [] },
      { pack_refs: [{}] },
      { owner_refs: [null] },
      { owner_types: [] },
      { owner_types: "pack" },
      { owner_types: ["pack", "unknown"] },
      { owner_types: ["PACK"] },
      { visibility: [] },
      { visibility: ["secret"] },
      { visibility: [false] },
      { visibility: {} },
      { encrypted: "true" },
      { encrypted: 0 },
      { encrypted: {} },
      { owner: ["any"] },
      { owner: "unknown" },
      { execution_scope: {} },
      { execution_scope: "unknown" },
      { attributes: [] },
    ];
    for (const constraints of invalid) {
      expect(() => parse({ grants: [{ ...grant, constraints }] })).toThrow();
    }
  });

  it("rejects human-relative fixed consent exactly as the server's stable-grant contract does", () => {
    for (const constraints of [
      { owner: "self" },
      { execution_scope: "self" },
      { execution_scope: "descendants" },
      { attributes: { team: "reader" } },
      { attributes: { deeply: { nested: [true] } } },
    ])
      expect(() => parse({ grants: [{ ...grant, constraints }] })).toThrow(
        /human-relative|attribute/,
      );
  });

  it("never removes malformed/expired expiry or substitutes a no-expiry approval", () => {
    for (const expires_at of [
      false,
      1,
      [],
      {},
      "",
      "not-a-date",
      "2099-01-01",
      "2099-01-01T00:00:00",
      "2000-01-01T00:00:00Z",
    ]) {
      expect(() => parse({ grants: [grant], expires_at })).toThrow();
    }
    expect(() => parseApprovalRequest("{malformed")).toThrow();
  });
});
