import { describe, expect, it } from "vitest";
import type { CurrentUserResponse } from "@/api";
import { hasPermission, requirementsForPath } from "@/lib/permissions";

function userWith(
  effectivePermissions: Array<{ resource: string; actions: string[] }>,
): CurrentUserResponse {
  return {
    id: 1,
    login: "alice",
    is_local: true,
    can_change_password: true,
    auth_provider: "local",
    assigned_permission_set_refs: [],
    effective_permissions: effectivePermissions,
  } as CurrentUserResponse;
}

describe("caches route permission requirement", () => {
  it("maps /caches routes to the caches resource", () => {
    expect(requirementsForPath("/caches")).toEqual([{ resource: "caches" }]);
    expect(requirementsForPath("/caches/pack/salesforce/users")).toEqual([
      { resource: "caches" },
    ]);
  });
});

describe("caches is not an authenticated default-read resource", () => {
  it("denies read access for an authenticated user with no explicit cache grant", () => {
    const user = userWith([{ resource: "packs", actions: ["read"] }]);
    // Unlike "packs"/"actions"/etc., every authenticated user does NOT get
    // implicit read access to caches — KEY_CACHE.md is explicit that caches
    // must not be added to the default authenticated read resources.
    expect(hasPermission(user, "caches", "read")).toBe(false);
  });

  it("grants read access once an explicit caches:read permission exists", () => {
    const user = userWith([{ resource: "caches", actions: ["read"] }]);
    expect(hasPermission(user, "caches", "read")).toBe(true);
  });

  it("still requires the write action explicitly for caches", () => {
    const user = userWith([{ resource: "caches", actions: ["read"] }]);
    expect(hasPermission(user, "caches", "update")).toBe(false);
    expect(hasPermission(user, "caches", "create")).toBe(false);
    expect(hasPermission(user, "caches", "delete")).toBe(false);
  });

  it("denies everything for an unauthenticated (null) user", () => {
    expect(hasPermission(null, "caches", "read")).toBe(false);
  });
});
