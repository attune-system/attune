import { describe, expect, it } from "vitest";
import {
  draftToGrant,
  grantToDraft,
  newGrantDraft,
  normalizeDraft,
  OWNER_REF_RESOURCES,
  OWNER_TYPE_RESOURCES,
  RESOURCE_ACTIONS,
} from "@/pages/access-control/grantDraft";
import type { ParsedGrant } from "@/components/access-control/grants";

describe("caches resource action set", () => {
  it("exposes exactly read/create/update/delete for caches", () => {
    // KEY_CACHE.md: "Read for entries and scans, Create for namespaces and
    // staging generations, Update for ingestion and promotion, and Delete for
    // explicit deletion."
    expect(RESOURCE_ACTIONS.caches).toEqual([
      "read",
      "create",
      "update",
      "delete",
    ]);
  });

  it("registers caches as both an owner-type- and owner-ref-scoped resource", () => {
    expect(OWNER_TYPE_RESOURCES.has("caches")).toBe(true);
    expect(OWNER_REF_RESOURCES.has("caches")).toBe(true);
  });
});

describe("grantToDraft / draftToGrant round trip for caches", () => {
  it("round-trips a namespace-scoped cache grant with owner type + owner ref", () => {
    const grant: ParsedGrant = {
      resource: "caches",
      actions: ["read"],
      constraints: {
        owner_types: ["pack"],
        owner_refs: ["salesforce"],
        refs: ["users"],
      },
    };

    const draft = grantToDraft(grant, 0);
    expect(draft.resource).toBe("caches");
    expect(draft.scopeType).toBe("component");
    expect(draft.scopeRefs).toBe("users");
    expect(draft.ownerTypes).toBe("pack");
    expect(draft.ownerRefs).toBe("salesforce");

    const rebuilt = draftToGrant(draft);
    expect(rebuilt).toEqual(grant);
  });

  it("supports an owner-only cache grant covering every namespace in that owner", () => {
    const grant: ParsedGrant = {
      resource: "caches",
      actions: ["create", "delete", "read", "update"],
      constraints: {
        owner_types: ["pack"],
        owner_refs: ["salesforce"],
      },
    };

    const draft = grantToDraft(grant, 0);
    expect(draft.scopeType).toBe("unconstrained");
    expect(draft.ownerTypes).toBe("pack");
    expect(draft.ownerRefs).toBe("salesforce");

    const rebuilt = draftToGrant(draft);
    expect(rebuilt).toEqual(grant);
  });

  it("does not leak owner_refs onto resources outside OWNER_REF_RESOURCES", () => {
    const draft = newGrantDraft();
    draft.resource = "keys";
    draft.ownerRefs = "salesforce";
    draft.actions = ["read"];

    const normalized = normalizeDraft(draft);
    expect(normalized.ownerRefs).toBe("");

    const grant = draftToGrant(normalized);
    expect(grant.constraints?.owner_refs).toBeUndefined();
  });

  it("clears owner_refs when normalizing after switching away from caches", () => {
    let draft = newGrantDraft();
    draft = normalizeDraft({
      ...draft,
      resource: "caches",
      actions: ["read"],
      ownerRefs: "salesforce",
    });
    expect(draft.ownerRefs).toBe("salesforce");

    const switched = normalizeDraft({ ...draft, resource: "actions" });
    expect(switched.ownerRefs).toBe("");
  });

  it("scopes the namespace refs field as a 'component' scope using the refs constraint", () => {
    const draft = normalizeDraft({
      ...newGrantDraft(),
      resource: "caches",
      actions: ["read"],
      scopeType: "component",
      scopeRefs: "users, locations",
    });

    const grant = draftToGrant(draft);
    expect(grant.constraints?.refs).toEqual(["users", "locations"]);
    // Cache grants never use pack_refs — owner scoping goes through
    // owner_types/owner_refs instead.
    expect(grant.constraints?.pack_refs).toBeUndefined();
  });

  it("rejects a pack-scoped cache grant (caches are not in PACK_SCOPED_RESOURCES)", () => {
    const draft = {
      ...newGrantDraft(),
      resource: "caches",
      actions: ["read"],
      scopeType: "pack" as const,
      scopeRefs: "salesforce",
    };

    expect(() => draftToGrant(draft)).toThrow(/cannot be pack scoped/);
  });
});
