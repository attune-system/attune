import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { RbacResource } from "@/api/models/RbacResource";
import { GrantsEditor } from "./GrantsEditor";
import { RESOURCE_META } from "./grants";
import { newGrantDraft } from "@/pages/access-control/grantDraft";
import grantOptions from "@/test/fixtures/permission-grant-options.json";

const resourceLabels: Record<RbacResource, string> = {
  packs: "Packs",
  actions: "Actions",
  policies: "Policies",
  queues: "Work Queues",
  queue_items: "Queue Items",
  rules: "Rules",
  triggers: "Triggers",
  executions: "Executions / Traces",
  events: "Events",
  enforcements: "Enforcements",
  inquiries: "Inquiries",
  keys: "Keys",
  caches: "Data Caches",
  artifacts: "Artifacts",
  runtimes: "Runtimes",
  workers: "Workers",
  dashboards: "Dashboards",
  retention: "Data Retention",
  identities: "Identities",
  permissions: "Permissions",
  audit_log: "Audit Log",
};

function Editor() {
  const [drafts, setDrafts] = useState([newGrantDraft()]);
  return <GrantsEditor drafts={drafts} onChange={setDrafts} />;
}

describe("permission grant resource options", () => {
  it("matches the backend-checked resource catalog exactly", () => {
    render(<Editor />);
    const resource = screen.getByRole("combobox", { name: "Resource" });
    const values = within(resource)
      .getAllByRole("option")
      .map((option) => option.getAttribute("value"));
    expect(values.sort()).toEqual(Object.keys(grantOptions).sort());
  });

  it.each(Object.entries(grantOptions))(
    "renders exactly the backend-validated actions for %s",
    async (resource, actions) => {
      const user = userEvent.setup();
      render(<Editor />);
      await user.selectOptions(
        screen.getByRole("combobox", { name: "Resource" }),
        resource,
      );
      const group = screen.getByRole("group", { name: "Permission specs" });
      const renderedActions = within(group)
        .getAllByRole("checkbox")
        .map((checkbox) => checkbox.closest("label")?.textContent?.trim());
      expect(renderedActions.sort()).toEqual(actions);
    },
  );

  it("offers every supported resource with its human-readable label", () => {
    render(<Editor />);
    const resource = screen.getByRole("combobox", { name: "Resource" });
    const options = within(resource).getAllByRole("option");
    expect(options).toHaveLength(Object.values(RbacResource).length);
    for (const [value, label] of Object.entries(resourceLabels)) {
      expect(
        within(resource).getByRole("option", { name: label, exact: true }),
      ).toHaveValue(value);
    }
    expect(options.map((option) => option.textContent)).toEqual(
      Object.values(resourceLabels).sort((a, b) => a.localeCompare(b)),
    );
  });

  it("defines display metadata for every supported resource and no obsolete ones", () => {
    expect(Object.keys(RESOURCE_META).sort()).toEqual(
      Object.values(RbacResource).sort(),
    );
  });

  it("lets an operator select and edit dashboard grants", async () => {
    const user = userEvent.setup();
    render(<Editor />);
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Resource" }),
      "dashboards",
    );
    expect(
      screen.getByRole("checkbox", { name: "read", exact: true }),
    ).toBeChecked();
    for (const action of ["create", "update", "delete"]) {
      const checkbox = screen.getByRole("checkbox", {
        name: action,
        exact: true,
      });
      await user.click(checkbox);
      expect(checkbox).toBeChecked();
    }
  });
});
