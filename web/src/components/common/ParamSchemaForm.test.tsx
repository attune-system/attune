import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import ParamSchemaForm from "./ParamSchemaForm";

afterEach(() => vi.restoreAllMocks());

it("keeps caller template namespaces in the shared renderer", () => {
  const onChange = vi.fn();
  render(
    <ParamSchemaForm
      schema={{ message: { type: "string" } }}
      contract={{
        annotated_schema: {
          type: "object",
          properties: {
            message: { type: "string" },
          },
        },
      }}
      values={{ message: "" }}
      onChange={onChange}
      allowTemplates
      templateNamespace="parameters"
    />,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Use template for /message" }),
  );
  expect(onChange).toHaveBeenCalledWith({ message: "{{ parameters.value }}" });
});

it("resolves primitive-only forms on the server and preserves explicit values", async () => {
  const schema = {
    enabled: { type: "boolean", default: true },
    count: { type: "integer", default: 7 },
    label: { type: "string", default: "fallback" },
  };
  const preview = vi.spyOn(SchemaTypesService, "previewSchema").mockReturnValue(
    new CancelablePromise<
      Awaited<ReturnType<typeof SchemaTypesService.previewSchema>>
    >((resolve) =>
      resolve({
        data: {
          semantic_digest: "primitive-preview",
          contract: {
            annotated_schema: {
              type: "object",
              properties: {
                enabled: { type: "boolean", title: "Resolved enabled" },
                count: { type: "integer" },
                label: { type: "string" },
              },
            },
          },
        },
      }),
    ),
  );
  const onChange = vi.fn();
  const onPreviewDigest = vi.fn();
  render(
    <ParamSchemaForm
      schema={schema}
      values={{ enabled: false, count: 0, label: "" }}
      onChange={onChange}
      onPreviewDigest={onPreviewDigest}
    />,
  );
  await waitFor(() =>
    expect(screen.getByText("Resolved enabled")).toBeInTheDocument(),
  );
  expect(screen.getByRole("checkbox")).not.toBeChecked();
  expect(screen.getByRole("spinbutton")).toHaveValue(0);
  expect(screen.getByRole("textbox")).toHaveValue("");
  expect(onChange).not.toHaveBeenCalled();
  expect(onPreviewDigest).toHaveBeenCalledWith("primitive-preview");
  expect(preview).toHaveBeenCalledWith({
    requestBody: { context: "flat_document", schema },
  });
});

it("uses supplied historical contracts without previewing the mutable schema", () => {
  const preview = vi.spyOn(SchemaTypesService, "previewSchema");
  render(
    <ParamSchemaForm
      schema={{ token: { type: "string" } }}
      contract={{
        annotated_schema: {
          type: "object",
          properties: {
            token: { type: "string", secret: true },
          },
        },
      }}
      values={{ token: "synthetic-private-value" }}
      onChange={vi.fn()}
    />,
  );
  expect(screen.getByLabelText("/token")).toHaveAttribute("type", "password");
  expect(preview).not.toHaveBeenCalled();
});
