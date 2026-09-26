import { describe, expect, it } from "vitest";

import {
  createEmptyDashboardDocument,
  dashboardDocumentToCreateRequest,
  dashboardDocumentToUpdateRequest,
} from "./dashboard";

describe("dashboard request scope fields", () => {
  it("omits scope_ref for global dashboards but retains visibility", () => {
    const document = createEmptyDashboardDocument();
    document.scope_ref = "stale-scope";
    document.visibility = "pack";

    const request = dashboardDocumentToCreateRequest(document);

    expect(request).not.toHaveProperty("scope_ref");
    expect(request.visibility).toBe("pack");
    expect(request.spec).not.toHaveProperty("scope_ref");
    expect(request.spec).not.toHaveProperty("visibility");
  });

  it("normalizes and includes the required pack scope ref", () => {
    const document = createEmptyDashboardDocument();
    document.scope_type = "pack";
    document.scope_ref = " My Pack ";

    const request = dashboardDocumentToCreateRequest(document);

    expect(request.scope_ref).toBe("my_pack");
    expect(request.visibility).toBe("public");
  });

  it("omits stale hidden identity fields from create and preview payloads", () => {
    const document = createEmptyDashboardDocument();
    document.scope_type = "identity";
    document.scope_ref = "someone-else";
    document.visibility = "public";
    document.spec.scope_ref = "someone-else";
    document.spec.visibility = "public";

    const request = dashboardDocumentToCreateRequest(document);

    expect(request).not.toHaveProperty("scope_ref");
    expect(request).not.toHaveProperty("visibility");
    expect(request.spec).not.toHaveProperty("scope_ref");
    expect(request.spec).not.toHaveProperty("visibility");
  });

  it("omits stale hidden identity fields from update payloads", () => {
    const document = createEmptyDashboardDocument();
    document.revision = 7;
    document.scope_type = "identity";
    document.scope_ref = "someone-else";
    document.visibility = "pack";
    document.spec.scope_ref = "someone-else";
    document.spec.visibility = "pack";

    const request = dashboardDocumentToUpdateRequest(document);

    expect(request.expected_revision).toBe(7);
    expect(request).not.toHaveProperty("scope_ref");
    expect(request).not.toHaveProperty("visibility");
    expect(request.spec).not.toHaveProperty("scope_ref");
    expect(request.spec).not.toHaveProperty("visibility");
  });

  it("omits a stale global scope ref from update payloads", () => {
    const document = createEmptyDashboardDocument();
    document.revision = 3;
    document.scope_ref = "stale-pack";

    const request = dashboardDocumentToUpdateRequest(document);

    expect(request).not.toHaveProperty("scope_ref");
  });
});
