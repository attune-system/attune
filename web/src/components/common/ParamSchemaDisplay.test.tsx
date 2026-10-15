import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import ParamSchemaDisplay from "./ParamSchemaDisplay";

afterEach(() => vi.restoreAllMocks());

it("renders named rule selector annotations through the bounded shared display", async () => {
  vi.spyOn(SchemaTypesService, "previewSchema").mockReturnValue(
    new CancelablePromise<
      Awaited<ReturnType<typeof SchemaTypesService.previewSchema>>
    >((resolve) =>
      resolve({
        data: {
          semantic_digest: "owned-preview",
          contract: {
            annotated_schema: {
              type: "object",
              properties: {
                nested: { $ref: "#/$defs/nested" },
                count: { type: "integer" },
              },
              $defs: {
                nested: {
                  type: "object",
                  properties: {
                    token: { type: "string", secret: true },
                    enabled: { type: "boolean" },
                  },
                },
              },
            },
          },
        },
      }),
    ),
  );
  render(
    <ParamSchemaDisplay
      schema={{ nested: { type: "demo.nested" }, count: { type: "integer" } }}
      values={{
        nested: { token: "private-selector-fixture", enabled: false },
        count: 0,
      }}
    />,
  );
  expect(await screen.findByText("[REDACTED]")).toBeInTheDocument();
  expect(screen.getByText("false")).toBeInTheDocument();
  expect(screen.getByText("0")).toBeInTheDocument();
  expect(
    screen.queryByText("private-selector-fixture"),
  ).not.toBeInTheDocument();
});

it("keeps values hidden when named resolution fails", async () => {
  vi.spyOn(SchemaTypesService, "previewSchema").mockReturnValue(
    new CancelablePromise((_, reject) => reject(new Error("unavailable"))),
  );
  render(
    <ParamSchemaDisplay
      schema={{ value: { type: "demo.secret" } }}
      values={{ value: "private-unresolved-fixture" }}
    />,
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Values remain hidden",
  );
  expect(
    screen.queryByText("private-unresolved-fixture"),
  ).not.toBeInTheDocument();
});
