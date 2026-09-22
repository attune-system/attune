import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { Link2, Pencil, Plus, Trash2 } from "lucide-react";
import type {
  CreateExternalIdentityMappingRequest,
  ExternalIdentityMappingResponse,
} from "@/api";
import {
  useCreateExternalIdentityMapping,
  useDeleteExternalIdentityMapping,
  useExternalIdentityMappings,
  useUpdateExternalIdentityMapping,
} from "@/hooks/useExternalIdentityMappings";
import { safeErrorSummary } from "@/lib/safe-error-summary";

interface MappingFormValues {
  mappedIdentity: string;
  provider: string;
  tenant: string;
  subjectKind: string;
  externalSubject: string;
}

type FormState =
  | { kind: "closed" }
  | { kind: "create"; values: MappingFormValues }
  | {
      kind: "edit";
      mappingId: number;
      values: MappingFormValues;
    };

const EMPTY_VALUES: MappingFormValues = {
  mappedIdentity: "",
  provider: "",
  tenant: "",
  subjectKind: "user",
  externalSubject: "",
};

function valuesFromMapping(
  mapping: ExternalIdentityMappingResponse,
): MappingFormValues {
  return {
    mappedIdentity: String(mapping.mapped_identity),
    provider: mapping.provider,
    tenant: mapping.tenant,
    subjectKind: mapping.subject_kind,
    externalSubject: mapping.external_subject,
  };
}

function requestFromValues(
  values: MappingFormValues,
): CreateExternalIdentityMappingRequest | null {
  const mappedIdentity = Number(values.mappedIdentity);
  if (
    !Number.isSafeInteger(mappedIdentity) ||
    mappedIdentity < 1 ||
    !values.provider.trim() ||
    !values.tenant.trim() ||
    !values.subjectKind.trim() ||
    !values.externalSubject.trim()
  ) {
    return null;
  }

  return {
    mapped_identity: mappedIdentity,
    provider: values.provider.trim(),
    tenant: values.tenant.trim(),
    subject_kind: values.subjectKind.trim(),
    external_subject: values.externalSubject.trim(),
  };
}

function mappingErrorMessage(error: unknown): string {
  const summary = safeErrorSummary(error);
  return summary.status
    ? `External identity mapping request failed with HTTP ${summary.status}.`
    : "External identity mapping request failed.";
}

export function ExternalIdentityMappingsSection({
  integrationIdentity,
}: {
  integrationIdentity: number;
}) {
  const [page, setPage] = useState(1);
  const [form, setForm] = useState<FormState>({ kind: "closed" });
  const [formError, setFormError] = useState<string | null>(null);
  const { data, isLoading, error } = useExternalIdentityMappings(
    integrationIdentity,
    page,
  );
  const createMutation = useCreateExternalIdentityMapping();
  const updateMutation = useUpdateExternalIdentityMapping();
  const deleteMutation = useDeleteExternalIdentityMapping();
  const mappings: ExternalIdentityMappingResponse[] = data?.items ?? [];
  const isSaving = createMutation.isPending || updateMutation.isPending;

  const updateForm = (field: keyof MappingFormValues, value: string) => {
    setForm((current) =>
      current.kind === "closed"
        ? current
        : { ...current, values: { ...current.values, [field]: value } },
    );
  };

  const closeForm = () => {
    setForm({ kind: "closed" });
    setFormError(null);
  };

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault();
    if (form.kind === "closed") return;
    const requestBody = requestFromValues(form.values);
    if (!requestBody) {
      setFormError("Complete every field with a valid Attune identity ID.");
      return;
    }

    setFormError(null);
    try {
      if (form.kind === "create") {
        await createMutation.mutateAsync({
          integrationIdentity,
          requestBody,
        });
      } else {
        await updateMutation.mutateAsync({
          integrationIdentity,
          mappingId: form.mappingId,
          requestBody,
        });
      }
      closeForm();
    } catch (submissionError) {
      setFormError(mappingErrorMessage(submissionError));
    }
  };

  const handleDelete = async (mapping: ExternalIdentityMappingResponse) => {
    if (
      !window.confirm(
        `Delete the ${mapping.provider} mapping for ${mapping.external_subject}?`,
      )
    ) {
      return;
    }
    setFormError(null);
    try {
      await deleteMutation.mutateAsync({
        integrationIdentity,
        mappingId: mapping.id,
      });
    } catch (deletionError) {
      setFormError(mappingErrorMessage(deletionError));
    }
  };

  return (
    <section className="rounded-lg bg-white p-6 shadow">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <Link2 className="h-5 w-5 text-cyan-600" />
            <h2 className="text-lg font-semibold text-gray-900">
              External identity mappings
            </h2>
            <span className="text-sm text-gray-500">
              ({data?.pagination.total_items ?? mappings.length})
            </span>
          </div>
          <p className="mt-1 text-sm text-gray-500">
            Resolve provider accounts to Attune identities for this integration.
          </p>
        </div>
        <button
          type="button"
          onClick={() => {
            setForm({ kind: "create", values: EMPTY_VALUES });
            setFormError(null);
          }}
          className="inline-flex shrink-0 items-center gap-1 rounded-lg bg-cyan-700 px-3 py-1.5 text-sm text-white transition-colors hover:bg-cyan-800"
        >
          <Plus className="h-4 w-4" /> Add mapping
        </button>
      </div>

      {form.kind !== "closed" && (
        <form
          onSubmit={handleSubmit}
          className="mt-4 rounded-lg border border-cyan-100 bg-cyan-50/50 p-4"
        >
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="text-xs font-medium uppercase tracking-wide text-gray-600">
              Provider
              <input
                value={form.values.provider}
                onChange={(event) => updateForm("provider", event.target.value)}
                required
                maxLength={64}
                placeholder="slack"
                className="mt-1 w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm normal-case tracking-normal focus:outline-none focus:ring-2 focus:ring-cyan-600"
              />
            </label>
            <label className="text-xs font-medium uppercase tracking-wide text-gray-600">
              Tenant
              <input
                value={form.values.tenant}
                onChange={(event) => updateForm("tenant", event.target.value)}
                required
                maxLength={255}
                placeholder="Workspace or tenant ID"
                className="mt-1 w-full rounded-md border border-gray-300 bg-white px-3 py-2 font-mono text-sm normal-case tracking-normal focus:outline-none focus:ring-2 focus:ring-cyan-600"
              />
            </label>
            <label className="text-xs font-medium uppercase tracking-wide text-gray-600">
              Subject kind
              <input
                value={form.values.subjectKind}
                onChange={(event) =>
                  updateForm("subjectKind", event.target.value)
                }
                required
                maxLength={64}
                placeholder="user"
                className="mt-1 w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm normal-case tracking-normal focus:outline-none focus:ring-2 focus:ring-cyan-600"
              />
            </label>
            <label className="text-xs font-medium uppercase tracking-wide text-gray-600">
              External subject
              <input
                value={form.values.externalSubject}
                onChange={(event) =>
                  updateForm("externalSubject", event.target.value)
                }
                required
                maxLength={255}
                placeholder="Provider user ID"
                className="mt-1 w-full rounded-md border border-gray-300 bg-white px-3 py-2 font-mono text-sm normal-case tracking-normal focus:outline-none focus:ring-2 focus:ring-cyan-600"
              />
            </label>
            <label className="text-xs font-medium uppercase tracking-wide text-gray-600 sm:col-span-2">
              Mapped Attune identity ID
              <input
                type="number"
                min={1}
                step={1}
                value={form.values.mappedIdentity}
                onChange={(event) =>
                  updateForm("mappedIdentity", event.target.value)
                }
                required
                placeholder="42"
                className="mt-1 w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm normal-case tracking-normal focus:outline-none focus:ring-2 focus:ring-cyan-600"
              />
            </label>
          </div>
          {formError && (
            <p role="alert" className="mt-3 text-sm text-red-700">
              {formError}
            </p>
          )}
          <div className="mt-4 flex justify-end gap-2">
            <button
              type="button"
              onClick={closeForm}
              className="px-3 py-2 text-sm text-gray-600 hover:text-gray-900"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={isSaving}
              className="rounded-lg bg-cyan-700 px-4 py-2 text-sm text-white hover:bg-cyan-800 disabled:opacity-50"
            >
              {isSaving
                ? "Saving..."
                : form.kind === "create"
                  ? "Create mapping"
                  : "Save changes"}
            </button>
          </div>
        </form>
      )}

      {form.kind === "closed" && formError && (
        <p role="alert" className="mt-4 text-sm text-red-700">
          {formError}
        </p>
      )}

      {isLoading ? (
        <p className="mt-6 text-center text-sm text-gray-500">
          Loading external identity mappings...
        </p>
      ) : error ? (
        <p role="alert" className="mt-6 text-center text-sm text-red-700">
          {mappingErrorMessage(error)}
        </p>
      ) : mappings.length > 0 ? (
        <div className="mt-4 divide-y divide-gray-100">
          {mappings.map((mapping) => (
            <div
              key={mapping.id}
              className="flex items-start justify-between gap-4 py-3"
            >
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="rounded-full bg-cyan-100 px-2 py-0.5 text-xs font-semibold text-cyan-800">
                    {mapping.provider}
                  </span>
                  <span className="text-xs text-gray-500">
                    {mapping.subject_kind}
                  </span>
                </div>
                <div className="mt-2 grid gap-x-6 gap-y-1 text-sm sm:grid-cols-2">
                  <div className="min-w-0">
                    <span className="text-gray-500">Tenant </span>
                    <span className="break-all font-mono text-gray-900">
                      {mapping.tenant}
                    </span>
                  </div>
                  <div className="min-w-0">
                    <span className="text-gray-500">Subject </span>
                    <span className="break-all font-mono text-gray-900">
                      {mapping.external_subject}
                    </span>
                  </div>
                </div>
                <Link
                  to={`/access-control/identities/${mapping.mapped_identity}`}
                  className="mt-2 inline-flex text-sm font-medium text-blue-700 hover:underline"
                >
                  Attune identity {mapping.mapped_identity}
                </Link>
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <button
                  type="button"
                  onClick={() => {
                    setForm({
                      kind: "edit",
                      mappingId: mapping.id,
                      values: valuesFromMapping(mapping),
                    });
                    setFormError(null);
                  }}
                  className="p-1 text-gray-400 hover:text-cyan-700"
                  aria-label={`Edit ${mapping.provider} mapping`}
                >
                  <Pencil className="h-4 w-4" />
                </button>
                <button
                  type="button"
                  onClick={() => handleDelete(mapping)}
                  disabled={deleteMutation.isPending}
                  className="p-1 text-red-400 hover:text-red-600 disabled:opacity-50"
                  aria-label={`Delete ${mapping.provider} mapping`}
                >
                  <Trash2 className="h-4 w-4" />
                </button>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="py-8 text-center">
          <Link2 className="mx-auto h-8 w-8 text-gray-300" />
          <p className="mt-2 text-sm text-gray-500">
            No external identity mappings
          </p>
          <p className="mt-1 text-xs text-gray-400">
            Add one when this identity receives callbacks from an external
            provider.
          </p>
        </div>
      )}

      {data && (data.pagination.has_previous || data.pagination.has_next) && (
        <div className="mt-4 flex items-center justify-between border-t border-gray-100 pt-4">
          <button
            type="button"
            disabled={!data.pagination.has_previous}
            onClick={() => setPage((current) => Math.max(1, current - 1))}
            className="text-sm text-gray-600 hover:text-gray-900 disabled:opacity-40"
          >
            Previous
          </button>
          <span className="text-xs text-gray-500">
            Page {data.pagination.page}
          </span>
          <button
            type="button"
            disabled={!data.pagination.has_next}
            onClick={() => setPage((current) => current + 1)}
            className="text-sm text-gray-600 hover:text-gray-900 disabled:opacity-40"
          >
            Next
          </button>
        </div>
      )}
    </section>
  );
}
