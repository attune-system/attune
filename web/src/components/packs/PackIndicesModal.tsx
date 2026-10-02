import { useEffect, useRef, useState, type FormEvent } from "react";
import { GripVertical, RefreshCw, X } from "lucide-react";
import { PacksService } from "@/api";
import {
  useCreatePackIndex,
  useDeletePackIndex,
  usePackIndices,
  useRefreshPackIndex,
  useUpdatePackIndex,
} from "@/hooks/usePacks";

type PackIndex = Awaited<
  ReturnType<typeof PacksService.listPackIndices>
>["data"][number];
type FormFields = {
  name: string;
  url: string;
  enabled: boolean;
  headers: string;
};
type IndexForm = FormFields &
  ({ kind: "create" } | { kind: "edit"; id: number });
type Feedback = { kind: "success" | "error"; text: string };

function newForm(): IndexForm {
  return { kind: "create", name: "", url: "", enabled: true, headers: "{}" };
}

function parseHeaders(text: string): Record<string, string> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw new Error(
      "HTTP headers must be a valid JSON object with string values.",
    );
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("HTTP headers must be a JSON object with string values.");
  }
  const entries: [string, string][] = [];
  for (const [name, header] of Object.entries(value)) {
    if (typeof header !== "string") {
      throw new Error("HTTP header values must all be strings.");
    }
    entries.push([name, header]);
  }
  return Object.fromEntries(entries);
}

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "body" in error) {
    const body: unknown = error.body;
    if (
      typeof body === "object" &&
      body !== null &&
      "error" in body &&
      typeof body.error === "string"
    ) {
      return body.error;
    }
  }
  return error instanceof Error ? error.message : "The index operation failed.";
}

function isPinnedUrl(url: string): boolean {
  try {
    return new URL(url).pathname
      .split("/")
      .some((part) => /^[a-f0-9]{40,64}$/i.test(part));
  } catch {
    return false;
  }
}

export default function PackIndicesModal({ onClose }: { onClose: () => void }) {
  const indices = usePackIndices();
  const createIndex = useCreatePackIndex();
  const updateIndex = useUpdatePackIndex();
  const deleteIndex = useDeletePackIndex();
  const refreshIndex = useRefreshPackIndex();
  const [form, setForm] = useState<IndexForm>(newForm);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [busy, setBusy] = useState(false);
  const [draggedId, setDraggedId] = useState<number | null>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  const feedbackElement = useRef<HTMLDivElement>(null);
  const configured = indices.data?.data ?? [];

  useEffect(() => {
    feedbackElement.current?.scrollIntoView?.({ block: "nearest" });
  }, [feedback]);

  async function perform(operation: () => Promise<string>) {
    setFeedback(null);
    setBusy(true);
    try {
      setFeedback({ kind: "success", text: await operation() });
    } catch (error) {
      setFeedback({ kind: "error", text: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  }

  function edit(index: PackIndex) {
    setFeedback(null);
    setForm({
      kind: "edit",
      id: index.id,
      name: index.name ?? "",
      url: index.url,
      enabled: index.enabled,
      headers: JSON.stringify(index.headers, null, 2),
    });
    nameInput.current?.focus();
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    await perform(async () => {
      const headers = parseHeaders(form.headers);
      const data = {
        name: form.name.trim() || null,
        url: form.url.trim(),
        enabled: form.enabled,
        headers,
      };
      if (form.kind === "edit") {
        await updateIndex.mutateAsync({ id: form.id, data });
      } else {
        if (Object.values(headers).includes("[REDACTED]")) {
          throw new Error(
            "Enter a value for each new HTTP header; [REDACTED] only preserves existing headers when editing.",
          );
        }
        await createIndex.mutateAsync(data);
      }
      const message =
        form.kind === "edit" ? "Index configuration saved." : "Index added.";
      setForm(newForm());
      return message;
    });
  }

  async function refresh(index: PackIndex) {
    await perform(async () => {
      const result = await refreshIndex.mutateAsync(index.id);
      return `Fetched ${result.data.length} pack entries from ${index.name || index.url}.`;
    });
  }

  async function toggle(index: PackIndex) {
    await perform(async () => {
      await updateIndex.mutateAsync({
        id: index.id,
        data: { enabled: !index.enabled },
      });
      setForm((current) =>
        current.kind === "edit" && current.id === index.id
          ? { ...current, enabled: !index.enabled }
          : current,
      );
      return index.enabled ? "Index disabled." : "Index enabled.";
    });
  }

  async function remove(index: PackIndex) {
    await perform(async () => {
      await deleteIndex.mutateAsync(index.id);
      if (form.kind === "edit" && form.id === index.id) {
        setForm(newForm());
      }
      return "Index deleted.";
    });
  }

  async function reorder(targetId: number) {
    const sourceId = draggedId;
    setDraggedId(null);
    if (sourceId === null || sourceId === targetId || busy) {
      return;
    }
    const reordered = [...configured];
    const from = reordered.findIndex((index) => index.id === sourceId);
    const to = reordered.findIndex((index) => index.id === targetId);
    if (from < 0 || to < 0) {
      return;
    }
    const [moved] = reordered.splice(from, 1);
    reordered.splice(to, 0, moved);
    await perform(async () => {
      for (const [position, index] of reordered.entries()) {
        if (index.position !== position) {
          await updateIndex.mutateAsync({ id: index.id, data: { position } });
        }
      }
      return "Index search order saved.";
    });
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="pack-indices-title"
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) {
            onClose();
          }
        }}
        className="w-full max-w-3xl max-h-[90vh] overflow-auto rounded-lg bg-white shadow-xl"
      >
        <div className="flex items-start justify-between border-b border-gray-200 p-5 gap-3">
          <div>
            <h2 id="pack-indices-title" className="text-xl font-semibold">
              Configured pack indices
            </h2>
            <p className="text-sm text-gray-600 mt-1">
              Drag indices to set search order. Duplicate pack refs resolve to
              the first enabled index containing the ref.
            </p>
          </div>
          <button
            type="button"
            disabled={busy}
            onClick={onClose}
            aria-label="Close index configuration"
            className="rounded-lg p-2 text-gray-500 hover:bg-gray-100 disabled:opacity-50"
          >
            <X className="h-5 w-5" />
          </button>
        </div>
        <div className="p-5 space-y-5">
          <p className="text-sm text-gray-600">
            Metadata is fetched from each index URL while browsing or
            installing. Refresh fetches that index now; it does not install or
            upgrade packs.
          </p>
          {feedback && (
            <div
              ref={feedbackElement}
              role={feedback.kind === "error" ? "alert" : "status"}
              className={`rounded-lg border p-3 text-sm ${feedback.kind === "error" ? "border-red-200 bg-red-50 text-red-800" : "border-green-200 bg-green-50 text-green-800"}`}
            >
              {feedback.text}
            </div>
          )}
          {indices.error && (
            <div
              role="alert"
              className="rounded-lg border border-red-200 bg-red-50 p-3 text-red-800"
            >
              {errorMessage(indices.error)}
            </div>
          )}
          <form
            onSubmit={save}
            className="space-y-3 border border-gray-200 rounded-lg p-4"
          >
            <h3 className="font-semibold">
              {form.kind === "edit" ? "Edit index" : "Add index"}
            </h3>
            <fieldset disabled={busy} className="space-y-3">
              <div>
                <label
                  htmlFor="index-name"
                  className="block text-sm font-medium mb-1"
                >
                  Index name
                </label>
                <input
                  id="index-name"
                  ref={nameInput}
                  autoFocus
                  type="text"
                  value={form.name}
                  onChange={(event) =>
                    setForm({ ...form, name: event.target.value })
                  }
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg"
                />
              </div>
              <div>
                <label
                  htmlFor="index-url"
                  className="block text-sm font-medium mb-1"
                >
                  Index URL
                </label>
                <input
                  id="index-url"
                  type="url"
                  required
                  pattern="https://.*"
                  value={form.url}
                  onChange={(event) =>
                    setForm({ ...form, url: event.target.value })
                  }
                  placeholder="https://registry.example.com/index.json"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg"
                />
                <p className="text-xs text-gray-500 mt-1">
                  Use the direct HTTPS URL of the index JSON, not a Git
                  repository page. A raw-file URL selects the branch or commit
                  containing the index.
                </p>
              </div>
              <div>
                <label
                  htmlFor="index-headers"
                  className="block text-sm font-medium mb-1"
                >
                  HTTP headers
                </label>
                <textarea
                  id="index-headers"
                  rows={4}
                  spellCheck={false}
                  autoComplete="off"
                  value={form.headers}
                  onChange={(event) =>
                    setForm({ ...form, headers: event.target.value })
                  }
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg font-mono text-sm"
                />
                <p className="text-xs text-gray-500 mt-1">
                  JSON object with string values, for example{" "}
                  {`{"Authorization":"Bearer token"}`}. Stored values are
                  encrypted and displayed as [REDACTED]. Keep that placeholder
                  to preserve a value, or use {"{}"} to remove all headers.
                </p>
              </div>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={form.enabled}
                  onChange={(event) =>
                    setForm({ ...form, enabled: event.target.checked })
                  }
                />
                Enabled for pack discovery and installation
              </label>
              <div className="flex items-center gap-3">
                <button
                  type="submit"
                  className="px-4 py-2 bg-blue-600 text-white rounded-lg disabled:opacity-50"
                >
                  {busy
                    ? "Working..."
                    : form.kind === "edit"
                      ? "Save index"
                      : "Add Index"}
                </button>
                {form.kind === "edit" && (
                  <button
                    type="button"
                    onClick={() => {
                      setForm(newForm());
                      setFeedback(null);
                    }}
                    className="text-gray-600"
                  >
                    Cancel editing
                  </button>
                )}
              </div>
            </fieldset>
          </form>
          <div className="space-y-2" aria-busy={busy}>
            {configured.map((index, position) => (
              <div
                key={index.id}
                onDragOver={(event) => event.preventDefault()}
                onDrop={() => void reorder(index.id)}
                className={`border rounded-lg p-3 text-sm ${draggedId === index.id ? "border-blue-300 bg-blue-50" : "border-gray-200"}`}
              >
                <div className="flex items-start gap-3">
                  <button
                    type="button"
                    draggable={!busy}
                    disabled={busy}
                    onDragStart={(event) => {
                      event.dataTransfer.effectAllowed = "move";
                      event.dataTransfer.setData(
                        "text/plain",
                        String(index.id),
                      );
                      setDraggedId(index.id);
                    }}
                    onDragEnd={() => setDraggedId(null)}
                    aria-label={`Drag to reorder ${index.name || index.url}`}
                    className="rounded p-1 text-gray-400 cursor-grab"
                  >
                    <GripVertical className="h-5 w-5" />
                  </button>
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="font-medium">
                      {index.name || index.url}{" "}
                      {!index.enabled && (
                        <span className="text-gray-500">Disabled</span>
                      )}
                    </div>
                    <div className="text-gray-500 break-all">{index.url}</div>
                    <div className="text-xs text-gray-500">
                      {position === 0
                        ? "Checked first"
                        : "Checked after earlier indices"}
                      . Headers:{" "}
                      {Object.keys(index.headers).join(", ") || "none"}.
                    </div>
                    {isPinnedUrl(index.url) && (
                      <p className="text-xs text-amber-800">
                        Commit-pinned URL: refresh reads the same snapshot. Edit
                        the URL to select newer metadata.
                      </p>
                    )}
                    <div className="flex flex-wrap gap-3 pt-1">
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => edit(index)}
                        aria-label={`Edit ${index.name || index.url}`}
                        className="text-blue-600 disabled:opacity-50"
                      >
                        Edit
                      </button>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => void refresh(index)}
                        aria-label={`Refresh ${index.name || index.url}`}
                        title="Fetch this index's metadata now"
                        className="inline-flex items-center gap-1 text-blue-600 disabled:opacity-50"
                      >
                        <RefreshCw
                          className={`h-4 w-4 ${refreshIndex.isPending && refreshIndex.variables === index.id ? "animate-spin" : ""}`}
                        />
                        Refresh
                      </button>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => void toggle(index)}
                        className="text-blue-600 disabled:opacity-50"
                      >
                        {index.enabled ? "Disable" : "Enable"}
                      </button>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => void remove(index)}
                        className="text-red-600 disabled:opacity-50"
                      >
                        Delete
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            ))}
            {indices.isPending && (
              <p className="text-sm text-gray-500">Loading indices...</p>
            )}
            {!indices.isPending &&
              !indices.error &&
              configured.length === 0 && (
                <p className="text-sm text-gray-500">
                  No API-managed indices configured yet.
                </p>
              )}
          </div>
        </div>
      </div>
    </div>
  );
}
