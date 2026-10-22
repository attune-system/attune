import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi, afterEach } from "vitest";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import ContractValueForm from "./ContractValueForm";
import { SchemaPreviewContext } from "@/api/models/SchemaPreviewContext";

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

function Form({
  initial,
  definition = contract,
  onSubmit,
}: {
  initial: unknown;
  definition?: unknown;
  onSubmit?: (value: unknown) => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit?.(value);
      }}
    >
      <ContractValueForm
        contract={definition}
        value={value}
        onChange={setValue}
      />
      <output aria-label="Stored value">{JSON.stringify(value)}</output>
      {onSubmit && <button type="submit">Submit values</button>}
    </form>
  );
}

describe("contract value controls", () => {
  afterEach(() => vi.restoreAllMocks());
  it("renders referenced string enums as selects without choosing a missing required value", () => {
    render(
      <Form
        initial={{}}
        definition={{
          annotated_schema: {
            type: "object",
            required: ["mode"],
            properties: { mode: { $ref: "#/$defs/mode" } },
            $defs: { mode: { type: "string", enum: ["interval", "cron", ""] } },
          },
        }}
      />,
    );
    const select = screen.getByRole("combobox", { name: "/mode" });
    expect(select).toBeRequired();
    expect(select).toHaveValue("");
    expect(screen.getByLabelText("Stored value")).toHaveTextContent("{}");
    fireEvent.change(select, { target: { value: "1" } });
    expect(screen.getByLabelText("Stored value")).toHaveTextContent(
      '{"mode":"cron"}',
    );
    fireEvent.change(select, { target: { value: "2" } });
    expect(screen.getByLabelText("Stored value")).toHaveTextContent(
      '{"mode":""}',
    );
  });
  it("preserves numeric, boolean, and null enum values without string coercion", () => {
    render(
      <Form
        initial={{ choice: null }}
        definition={{
          annotated_schema: {
            type: "object",
            properties: { choice: { enum: [null, 0, false, "0", "false"] } },
          },
        }}
      />,
    );
    const select = screen.getByRole("combobox", { name: "/choice" });
    expect(select).toHaveValue("0");
    for (const [index, expected] of [
      ["1", '{"choice":0}'],
      ["2", '{"choice":false}'],
      ["3", '{"choice":"0"}'],
      ["4", '{"choice":"false"}'],
      ["0", '{"choice":null}'],
    ]) {
      fireEvent.change(select, { target: { value: index } });
      expect(screen.getByLabelText("Stored value")).toHaveTextContent(expected);
    }
  });
  it("keeps optional enum fields absent until added and supports removal", () => {
    render(
      <Form
        initial={{}}
        definition={{
          annotated_schema: {
            type: "object",
            properties: {
              mode: { type: "string", enum: ["interval", "cron"] },
            },
          },
        }}
      />,
    );
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add mode" }));
    fireEvent.change(screen.getByRole("combobox", { name: "/mode" }), {
      target: { value: "1" },
    });
    expect(screen.getByLabelText("Stored value")).toHaveTextContent(
      '{"mode":"cron"}',
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove mode" }));
    expect(screen.getByLabelText("Stored value")).toHaveTextContent("{}");
  });
  it("retains enum field templates and disables read-only selects", () => {
    const onChange = vi.fn();
    const definition = {
      annotated_schema: {
        type: "object",
        properties: { mode: { type: "string", enum: ["interval", "cron"] } },
      },
    };
    const { rerender } = render(
      <ContractValueForm
        contract={definition}
        value={{ mode: "interval" }}
        onChange={onChange}
        allowTemplates
      />,
    );
    expect(screen.getByRole("combobox", { name: "/mode" })).toHaveValue("0");
    fireEvent.click(
      screen.getByRole("button", { name: "Use template for /mode" }),
    );
    expect(onChange).toHaveBeenCalledWith({
      mode: "{{ event.payload.value }}",
    });
    rerender(
      <ContractValueForm
        contract={definition}
        value={{ mode: "{{ event.payload.mode }}" }}
        onChange={onChange}
        allowTemplates
      />,
    );
    expect(screen.getByRole("textbox", { name: "/mode" })).toHaveValue(
      "{{ event.payload.mode }}",
    );
    rerender(
      <ContractValueForm
        contract={definition}
        value={{ mode: "interval" }}
        onChange={onChange}
        disabled
      />,
    );
    expect(screen.getByRole("combobox", { name: "/mode" })).toBeDisabled();
  });
  it("does not silently replace invalid stored enum values or expose secret enum options", () => {
    const onChange = vi.fn();
    render(
      <ContractValueForm
        contract={{
          annotated_schema: {
            type: "object",
            properties: {
              mode: { type: "string", enum: ["interval", "cron"] },
              secret: {
                type: "string",
                secret: true,
                enum: ["private-choice"],
              },
            },
          },
        }}
        value={{ mode: "legacy", secret: "private-choice" }}
        onChange={onChange}
      />,
    );
    expect(screen.getByRole("combobox", { name: "/mode" })).toHaveValue("");
    expect(screen.getByLabelText("/secret")).toHaveAttribute(
      "type",
      "password",
    );
    expect(screen.queryByText(/private-choice/)).not.toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });
  it("renders missing required fields immediately without inventing scalar values", () => {
    render(
      <Form
        initial={{}}
        definition={{
          annotated_schema: {
            type: "object",
            required: ["name", "count", "enabled"],
            properties: {
              name: { type: "string" },
              count: { type: "integer" },
              enabled: { type: "boolean" },
              optional: { type: "string" },
            },
          },
        }}
      />,
    );
    expect(screen.queryByRole("button", { name: "Add name" })).toBeNull();
    expect(screen.getByLabelText("/name")).not.toBeRequired();
    expect(screen.getByLabelText("/count")).toHaveValue(null);
    expect(screen.getByLabelText("/count")).toBeRequired();
    expect(screen.getByLabelText("/enabled")).toHaveValue("");
    expect(screen.getByLabelText("/enabled")).toBeRequired();
    expect(screen.getByLabelText("Stored value")).toHaveTextContent("{}");
    fireEvent.change(screen.getByLabelText("/name"), {
      target: { value: "Example" },
    });
    fireEvent.change(screen.getByLabelText("/count"), {
      target: { value: "0" },
    });
    fireEvent.change(screen.getByLabelText("/enabled"), {
      target: { value: "false" },
    });
    expect(screen.getByLabelText("Stored value")).toHaveTextContent(
      '{"name":"Example","count":0,"enabled":false}',
    );
    expect(screen.getByRole("button", { name: "Add optional" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Remove name" })).toBeNull();
  });
  it.each([undefined, 0])(
    "submits a persisted empty required string with minLength %s",
    (minLength) => {
      const onSubmit = vi.fn();
      render(
        <Form
          initial={{}}
          onSubmit={onSubmit}
          definition={{
            annotated_schema: {
              type: "object",
              required: ["name"],
              properties: { name: { type: "string", minLength } },
            },
          }}
        />,
      );
      expect(screen.getByLabelText("Stored value")).toHaveTextContent("{}");
      fireEvent.change(screen.getByLabelText("/name"), {
        target: { value: "temporary" },
      });
      fireEvent.change(screen.getByLabelText("/name"), {
        target: { value: "" },
      });
      expect(screen.getByLabelText("Stored value")).toHaveTextContent(
        '{"name":""}',
      );
      fireEvent.click(screen.getByRole("button", { name: "Submit values" }));
      expect(onSubmit).toHaveBeenCalledWith({ name: "" });
    },
  );
  it("blocks an empty required string when minLength forbids it", () => {
    const onSubmit = vi.fn();
    render(
      <Form
        initial={{ name: "" }}
        onSubmit={onSubmit}
        definition={{
          annotated_schema: {
            type: "object",
            required: ["name"],
            properties: { name: { type: "string", minLength: 1 } },
          },
        }}
      />,
    );
    expect(screen.getByLabelText("/name")).toBeRequired();
    fireEvent.click(screen.getByRole("button", { name: "Submit values" }));
    expect(onSubmit).not.toHaveBeenCalled();
  });
  it("persists and submits empty required containers without inventing optional values", () => {
    const onSubmit = vi.fn();
    render(
      <Form
        initial={{}}
        onSubmit={onSubmit}
        definition={{
          annotated_schema: {
            type: "object",
            required: ["details", "entries"],
            properties: {
              details: { $ref: "#/$defs/details" },
              entries: {
                type: "array",
                items: { type: "string" },
                maxItems: 0,
              },
              optional: { type: "string" },
            },
            $defs: { details: { type: "object", properties: {} } },
          },
        }}
      />,
    );
    expect(screen.getByLabelText("Stored value")).toHaveTextContent("{}");
    expect(screen.queryByRole("button", { name: "Add details" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add entries" })).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Use empty object for details" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Use empty list for entries" }),
    );
    expect(screen.getByLabelText("Stored value")).toHaveTextContent(
      '{"details":{},"entries":[]}',
    );
    expect(screen.getByRole("button", { name: "Add optional" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Remove details" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Remove entries" })).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Use empty object for details" }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Use empty list for entries" }),
    ).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Submit values" }));
    expect(onSubmit).toHaveBeenCalledWith({ details: {}, entries: [] });
  });
  it("opens required referenced objects and lists while keeping nested optional fields absent", () => {
    render(
      <Form
        initial={{}}
        definition={{
          annotated_schema: {
            type: "object",
            required: ["details", "entries"],
            properties: {
              details: { $ref: "#/$defs/details" },
              entries: { type: "array", items: { type: "string" } },
            },
            $defs: {
              details: {
                type: "object",
                required: ["token"],
                properties: {
                  token: { type: "string", secret: true },
                  note: { type: "string" },
                },
              },
            },
          },
        }}
      />,
    );
    expect(screen.queryByRole("button", { name: "Add details" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add entries" })).toBeNull();
    expect(screen.getByLabelText("/details/token")).toHaveAttribute(
      "type",
      "password",
    );
    fireEvent.change(screen.getByLabelText("/details/token"), {
      target: { value: "fixture-token" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add item" }));
    expect(screen.getByLabelText("Stored value")).toHaveTextContent(
      '{"details":{"token":"fixture-token"},"entries":[""]}',
    );
    expect(screen.getByRole("button", { name: "Add note" })).toBeVisible();
  });
  it("omits whole-document templates but keeps field templates", () => {
    const onChange = vi.fn();
    render(
      <ContractValueForm
        contract={{
          annotated_schema: {
            type: "object",
            required: ["name"],
            properties: { name: { type: "string" } },
          },
        }}
        value={{}}
        onChange={onChange}
        allowTemplates
      />,
    );
    expect(
      screen.queryByRole("button", { name: "Use template for value" }),
    ).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Use template for /name" }),
    );
    expect(onChange).toHaveBeenCalledWith({
      name: "{{ event.payload.value }}",
    });
  });
  it("retains root template controls for single-value forms", () => {
    const onChange = vi.fn();
    render(
      <ContractValueForm
        contract={{ annotated_schema: { type: "string" } }}
        context={SchemaPreviewContext.SINGLE_VALUE}
        value=""
        onChange={onChange}
        allowTemplates
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Use template for value" }),
    );
    expect(onChange).toHaveBeenCalledWith("{{ event.payload.value }}");
  });
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
