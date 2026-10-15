import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import type { ApiResponse_SchemaContractResponse } from "@/api/models/ApiResponse_SchemaContractResponse";
import ValueSchemaEditor from "./ValueSchemaEditor";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("single-value schema editing", () => {
  it("keeps references authored and compares previews without selecting revisions", async () => {
    const digest = vi.fn();
    const change = vi.fn();
    const preview = vi
      .spyOn(SchemaTypesService, "previewSchema")
      .mockImplementation(
        () =>
          new CancelablePromise((resolve) =>
            resolve({
              data: {
                contract: { annotated_schema: { $ref: "#/$defs/n0" } },
                semantic_digest: "selected-digest",
              },
            }),
          ),
      );
    render(
      <ValueSchemaEditor
        value={{ type: "core.address" }}
        onChange={change}
        onError={vi.fn()}
        onDigest={digest}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Preview installed contract" }),
    );
    await waitFor(() => expect(digest).toHaveBeenCalledWith("selected-digest"));
    expect(preview).toHaveBeenCalledWith({
      requestBody: {
        context: "single_value",
        schema: { type: "core.address" },
      },
    });
    expect(change).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Schema for value"), {
      target: { value: '{"type":"core.other"}' },
    });
    expect(change).toHaveBeenCalledWith({ type: "core.other" });
    expect(digest).toHaveBeenLastCalledWith(undefined);
  });

  it("does not apply a preview completed after the authored schema changes", async () => {
    let resolve:
      ((response: ApiResponse_SchemaContractResponse) => void) | undefined;
    vi.spyOn(SchemaTypesService, "previewSchema").mockImplementation(
      () =>
        new CancelablePromise((complete) => {
          resolve = complete;
        }),
    );
    const digest = vi.fn();
    render(
      <ValueSchemaEditor
        value={{ type: "core.first" }}
        onChange={vi.fn()}
        onError={vi.fn()}
        onDigest={digest}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Preview installed contract" }),
    );
    fireEvent.change(screen.getByLabelText("Schema for value"), {
      target: { value: '{"type":"core.second"}' },
    });
    await act(async () => {
      resolve?.({ data: { contract: {}, semantic_digest: "stale" } });
    });
    expect(digest).not.toHaveBeenCalledWith("stale");
  });

  it("preserves false schemas and reports malformed input rather than clearing the assignment", () => {
    const change = vi.fn();
    const error = vi.fn();
    render(
      <ValueSchemaEditor
        value={false}
        onChange={change}
        onError={error}
        onDigest={vi.fn()}
      />,
    );
    expect(screen.getByLabelText("Schema for value")).toHaveValue("false");
    fireEvent.change(screen.getByLabelText("Schema for value"), {
      target: { value: "[" },
    });
    expect(change).not.toHaveBeenCalled();
    expect(error).toHaveBeenLastCalledWith(expect.any(String));
    fireEvent.change(screen.getByLabelText("Schema for value"), {
      target: { value: "" },
    });
    expect(change).toHaveBeenLastCalledWith(null);
  });
});
