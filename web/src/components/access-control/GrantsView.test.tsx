import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { GrantsView } from "@/components/access-control/GrantsView";
import type { ParsedGrant } from "@/components/access-control/grants";

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
