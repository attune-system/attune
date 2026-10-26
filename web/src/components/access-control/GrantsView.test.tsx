import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { GrantsView } from "@/components/access-control/GrantsView";
import {
  RESOURCE_META,
  type ParsedGrant,
} from "@/components/access-control/grants";
import { RbacResource } from "@/api/models/RbacResource";

describe("GrantsView resource labels", () => {
  it("renders an explicit resource glyph for every supported resource", () => {
    const grants = Object.values(RbacResource).map((resource) => ({
      resource,
      actions: ["read"],
    }));
    render(<GrantsView grants={grants} />);
    for (const resource of Object.values(RbacResource)) {
      const label = RESOURCE_META[resource].label;
      const row = screen.getByRole("row", { name: `${label} read` });
      expect(row.querySelector("svg.lucide")).toBeInTheDocument();
      expect(row.querySelector("svg.lucide-shield")).toBeNull();
    }
  });

  it.each([
    ["dashboards", "Dashboards"],
    ["policies", "Policies"],
    ["queue_items", "Queue Items"],
    ["queues", "Work Queues"],
    ["retention", "Data Retention"],
  ])("renders %s with a readable label", (resource, label) => {
    render(<GrantsView grants={[{ resource, actions: ["read"] }]} />);
    expect(screen.getByText(label)).toBeInTheDocument();
  });
});

describe("GrantsView caches rendering", () => {
  it("renders the Data Caches resource label and icon column for a caches grant", () => {
    const grants: ParsedGrant[] = [
      {
        resource: "caches",
        actions: ["read"],
        constraints: {
          owner_types: ["pack"],
          owner_refs: ["salesforce"],
          refs: ["users"],
        },
      },
    ];

    render(<GrantsView grants={grants} />);

    expect(screen.getByText("Data Caches")).toBeInTheDocument();
    expect(screen.getByText("read")).toBeInTheDocument();
  });

  it("renders an owner_refs constraint chip distinct from owner_types", () => {
    const grants: ParsedGrant[] = [
      {
        resource: "caches",
        actions: ["read"],
        constraints: {
          owner_types: ["pack"],
          owner_refs: ["salesforce"],
        },
      },
    ];

    render(<GrantsView grants={grants} />);

    expect(screen.getByText("Type: pack")).toBeInTheDocument();
    expect(screen.getByText("Owner ref: salesforce")).toBeInTheDocument();
  });

  it("never renders a value/entry payload — only resource, actions, and constraint refs", () => {
    const grants: ParsedGrant[] = [
      {
        resource: "caches",
        actions: ["read"],
        constraints: { refs: ["users"] },
      },
    ];

    const { container } = render(<GrantsView grants={grants} />);
    expect(container.textContent).not.toContain("secret-value");
  });

  it("shows the empty state when there are no grants", () => {
    render(<GrantsView grants={[]} emptyStateTitle="No grants defined" />);
    expect(screen.getByText("No grants defined")).toBeInTheDocument();
  });
});
