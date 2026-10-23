import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, expect, it, vi } from "vitest";
import { RuntimesService } from "@/api";
import type { RuntimeResponse } from "@/api";
import RuntimeForm from "./RuntimeForm";
import formRequests from "../../test/fixtures/form-requests.json";

const fixtures = vi.hoisted(() => ({
  packs: { items: [{ id: 1, ref: "demo", label: "Demo", version: "1.0.0" }] },
}));
vi.mock("@/hooks/usePacks", () => ({
  usePacks: () => ({ data: fixtures.packs }),
}));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const runtime: RuntimeResponse = {
  id: 1,
  ref: "demo.python",
  pack: 1,
  pack_ref: "demo",
  name: "Python",
  description: "Existing description",
  installation: { method: "system" },
  distributions: {},
  execution_config: {},
  created: "2026-10-14",
  updated: "2026-10-14",
};

function showForm() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <RuntimeForm initialData={runtime} isEditing />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

it("clears blank runtime fields with explicit clear operations", async () => {
  const update = vi
    .spyOn(RuntimesService, "updateRuntime")
    .mockResolvedValue({ data: runtime });
  showForm();
  fireEvent.change(screen.getByDisplayValue("Existing description"), {
    target: { value: "" },
  });
  fireEvent.change(screen.getByPlaceholderText(/Leave blank to clear/), {
    target: { value: "" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save Runtime" }));
  await waitFor(() => expect(update).toHaveBeenCalledOnce());
  expect(
    JSON.parse(JSON.stringify(update.mock.calls[0][0].requestBody)),
  ).toEqual(formRequests.runtimeClear);
});

it("sets populated runtime fields without create-only identity fields", async () => {
  const update = vi
    .spyOn(RuntimesService, "updateRuntime")
    .mockResolvedValue({ data: runtime });
  showForm();
  expect(screen.getByRole("combobox")).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Save Runtime" }));
  await waitFor(() => expect(update).toHaveBeenCalledOnce());
  const request = update.mock.calls[0][0];
  expect(request.ref).toBe(runtime.ref);
  expect(JSON.parse(JSON.stringify(request.requestBody))).toEqual(
    formRequests.runtimeSet,
  );
});
