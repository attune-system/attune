import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi, afterEach } from "vitest";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import ContractValueForm from "./ContractValueForm";

const contract = {
  annotated_schema: {
    $ref: "#/$defs/root",
    $defs: {
      root: {
        type: "object",
        properties: {
          optional: { $ref: "#/$defs/nested" },
          items: { type: "array", items: { $ref: "#/$defs/nested" } },
          enabled: { type: "boolean" },
        },
      },
      nested: {
        type: "object",
        properties: {
          token: { type: "string", secret: true },
          count: { type: "integer", default: 0 },
        },
      },
    },
  },
};

it("uses resolved annotation overrides and treats prototype names as missing JSON fields", () => {
  const onChange = vi.fn<(value: unknown) => void>();
  render(
    <ContractValueForm
      contract={{
        annotated_schema: {
          type: "object",
          properties: {
            late: { $ref: "#/$defs/label", title: "Later", position: 2 },
            early: {
              $ref: "#/$defs/label",
              title: "Use title",
              description: "Use description",
              position: 1,
            },
            constructor: {
              type: "string",
              title: "Constructor field",
              position: 0,
              default: "owned",
            },
          },
          $defs: {
            label: {
              type: "string",
              title: "Inherited title",
              description: "Inherited description",
              position: 10,
            },
          },
        },
      }}
      value={{ late: "L", early: "E" }}
      onChange={onChange}
    />,
  );
  expect(screen.getByText("Use description")).toBeInTheDocument();
  expect(
    screen
      .getByText("Use title")
      .compareDocumentPosition(screen.getByText("Later")) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).not.toBe(0);
  expect(screen.getAllByRole("button")[0]).toHaveTextContent(
    "Add Constructor field",
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Add Constructor field" }),
  );
  expect(onChange).toHaveBeenCalledWith({
    late: "L",
    early: "E",
    constructor: "owned",
  });
});

function Form({ initial }: { initial: unknown }) {
  const [value, setValue] = useState(initial);
  return (
    <>
      <ContractValueForm
        contract={contract}
        value={value}
        onChange={setValue}
      />
      <output aria-label="Stored value">{JSON.stringify(value)}</output>
    </>
  );
}

describe("contract value controls", () => {
  afterEach(() => vi.restoreAllMocks());
  it("reports preview semantics without passing any contract or revision selector", async () => {
    const preview = vi
      .spyOn(SchemaTypesService, "previewSchema")
      .mockReturnValue(
        new CancelablePromise<
          Awaited<ReturnType<typeof SchemaTypesService.previewSchema>>
        >((resolve) =>
          resolve({
            data: { contract, semantic_digest: "preview-fixture-digest" },
          }),
        ),
      );
    const digest = vi.fn();
    const schema = { enabled: { type: "boolean" } };
    render(
      <ContractValueForm
        schema={schema}
        value={{ enabled: false }}
        onChange={vi.fn()}
        onPreviewDigest={digest}
      />,
    );
    await waitFor(() =>
      expect(digest).toHaveBeenCalledWith("preview-fixture-digest"),
    );
    expect(preview).toHaveBeenCalledWith({
      requestBody: { context: "flat_document", schema },
    });
  });
  it("does not invent optional objects and preserves false and zero", () => {
    render(<Form initial={{ enabled: false }} />);
    expect(screen.getByLabelText("Stored value").textContent).toBe(
      '{"enabled":false}',
    );
    fireEvent.click(screen.getByRole("button", { name: "Add optional" }));
    fireEvent.click(screen.getByRole("button", { name: "Add count" }));
    expect(screen.getByLabelText("Stored value").textContent).toBe(
      '{"enabled":false,"optional":{"count":0}}',
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove optional" }));
    expect(screen.getByLabelText("Stored value").textContent).toBe(
      '{"enabled":false}',
    );
  });
  it("renders bounded arrays of objects and masks secret text leaves", () => {
    render(
      <Form initial={{ items: [{ token: "private-fixture", count: 0 }] }} />,
    );
    expect(screen.getByLabelText("/items/0/token").getAttribute("type")).toBe(
      "password",
    );
    fireEvent.change(screen.getByLabelText("/items/0/count"), {
      target: { value: "4" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add item" }));
    expect(screen.getByLabelText("Stored value").textContent).toBe(
      '{"items":[{"token":"private-fixture","count":4},{}]}',
    );
  });
  it("preserves explicit null through the JSON fallback", () => {
    render(<Form initial={{ optional: null }} />);
    expect(screen.getByLabelText("/optional").getAttribute("type")).toBe(
      "password",
    );
    expect(screen.getByLabelText("Stored value").textContent).toBe(
      '{"optional":null}',
    );
  });
  it("does not downgrade secrecy inherited through a reference", () => {
    render(
      <ContractValueForm
        contract={{
          annotated_schema: {
            $ref: "#/$defs/value",
            secret: false,
            $defs: { value: { type: "string", secret: true } },
          },
        }}
        value="secret-fixture"
        onChange={vi.fn()}
      />,
    );
    expect(screen.getByDisplayValue("secret-fixture")).toHaveAttribute(
      "type",
      "password",
    );
  });
});
