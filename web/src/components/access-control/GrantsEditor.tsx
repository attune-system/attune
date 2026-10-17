import { useId } from "react";
import { Plus, Shield, Trash2 } from "lucide-react";
import { ACTION_STYLE, RESOURCE_META } from "./grants";
import {
  COMPONENT_SCOPED_RESOURCES,
  componentScopeFieldLabel,
  componentScopeOptionLabel,
  componentScopePlaceholder,
  type GrantDraft,
  newGrantDraft,
  normalizeDraft,
  OWNER_REF_RESOURCES,
  OWNER_SCOPED_RESOURCES,
  OWNER_TYPE_RESOURCES,
  PACK_SCOPED_RESOURCES,
  RESOURCE_ACTIONS,
} from "@/pages/access-control/grantDraft";

const RESOURCE_OPTIONS = Object.keys(RESOURCE_ACTIONS).map((value) => ({
  value,
  label: RESOURCE_META[value]?.label ?? value,
}));

export function GrantsEditor({
  drafts,
  onChange,
}: {
  drafts: GrantDraft[];
  onChange: (drafts: GrantDraft[]) => void;
}) {
  const editorId = useId();
  const updateDraft = (id: string, patch: Partial<GrantDraft>) => {
    onChange(
      drafts.map((draft) =>
        draft.id === id ? normalizeDraft({ ...draft, ...patch }) : draft,
      ),
    );
  };

  const toggleAction = (draft: GrantDraft, action: string) => {
    updateDraft(draft.id, {
      actions: draft.actions.includes(action)
        ? draft.actions.filter((entry) => entry !== action)
        : [...draft.actions, action],
    });
  };

  return (
    <div className="space-y-4 p-4">
      {drafts.length === 0 ? (
        <div className="rounded-lg border border-dashed border-gray-300 p-8 text-center text-sm text-gray-500">
          No grants configured. Add a grant to begin.
        </div>
      ) : (
        drafts.map((draft, index) => {
          const fieldId = `${editorId}-${draft.id}`;
          const meta = RESOURCE_META[draft.resource];
          const Icon = meta?.icon ?? Shield;
          const validActions = RESOURCE_ACTIONS[draft.resource] ?? [];
          const canPackScope = PACK_SCOPED_RESOURCES.has(draft.resource);
          const canComponentScope = COMPONENT_SCOPED_RESOURCES.has(
            draft.resource,
          );
          const showOwner = OWNER_SCOPED_RESOURCES.has(draft.resource);
          const showOwnerType = OWNER_TYPE_RESOURCES.has(draft.resource);
          const showOwnerRefs = OWNER_REF_RESOURCES.has(draft.resource);
          const showVisibility = draft.resource === "artifacts";
          const showExecutionScope = draft.resource === "executions";
          const showEncrypted = draft.resource === "keys";
          return (
            <div
              key={draft.id}
              className="rounded-lg border border-gray-200 bg-gray-50 p-4"
            >
              <div className="mb-4 flex items-start justify-between gap-4">
                <div className="flex items-center gap-2">
                  <Icon
                    className={`h-5 w-5 ${meta?.color ?? "text-gray-500"}`}
                  />
                  <div>
                    <div className="text-sm font-semibold text-gray-900">
                      Grant {index + 1}
                    </div>
                    <div className="text-xs text-gray-500">
                      Select a resource, permission specs, and optional scope.
                    </div>
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() =>
                    onChange(drafts.filter((entry) => entry.id !== draft.id))
                  }
                  className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs text-red-600 hover:bg-red-50"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  Remove
                </button>
              </div>
              <div className="grid gap-4 lg:grid-cols-[14rem,1fr]">
                <div>
                  <label
                    htmlFor={`${fieldId}-resource`}
                    className="block text-xs font-medium uppercase tracking-wide text-gray-500"
                  >
                    Resource
                  </label>
                  <select
                    id={`${fieldId}-resource`}
                    value={draft.resource}
                    onChange={(event) => {
                      updateDraft(draft.id, {
                        resource: event.target.value,
                        actions: RESOURCE_ACTIONS[event.target.value]?.includes(
                          "read",
                        )
                          ? ["read"]
                          : (RESOURCE_ACTIONS[event.target.value] ?? []).slice(
                              0,
                              1,
                            ),
                        scopeType: "unconstrained",
                        scopeRefs: "",
                      });
                    }}
                    className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                  >
                    {RESOURCE_OPTIONS.map((resource) => (
                      <option key={resource.value} value={resource.value}>
                        {resource.label}
                      </option>
                    ))}
                  </select>
                </div>
                <div>
                  <div className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                    Permission specs
                  </div>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {validActions.map((action) => (
                      <label
                        key={action}
                        className={`inline-flex cursor-pointer items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium ${draft.actions.includes(action) ? (ACTION_STYLE[action] ?? "bg-gray-100 text-gray-700") : "bg-white text-gray-500 ring-1 ring-gray-200"}`}
                      >
                        <input
                          type="checkbox"
                          checked={draft.actions.includes(action)}
                          onChange={() => toggleAction(draft, action)}
                          className="h-3 w-3"
                        />
                        {action}
                      </label>
                    ))}
                  </div>
                </div>
              </div>
              <div className="mt-5 rounded-md border border-gray-200 bg-white p-4">
                <div className="mb-3 text-xs font-semibold uppercase tracking-wide text-gray-500">
                  Resource scope
                </div>
                <label className="mb-3 block text-xs text-gray-600">
                  Authority scope
                  <select
                    aria-label="Grant authority scope"
                    value={draft.authorityScope ?? "inherit"}
                    onChange={(event) => {
                      const value = event.target.value;
                      if (value === "pack" || value === "external")
                        updateDraft(draft.id, { authorityScope: value });
                      else if (value === "inherit")
                        updateDraft(draft.id, { authorityScope: undefined });
                    }}
                    className="mt-1 w-full rounded-md border border-gray-300 px-2 py-1.5 text-sm"
                  >
                    <option value="inherit">Use permission-set default</option>
                    <option value="pack">Local: actual owning pack only</option>
                    <option value="external">
                      External: recorded approval required for managed use
                    </option>
                  </select>
                </label>
                {canPackScope || canComponentScope ? (
                  <div className="grid gap-4 md:grid-cols-[14rem,1fr]">
                    <div>
                      <label
                        htmlFor={`${fieldId}-scope`}
                        className="block text-xs font-medium uppercase tracking-wide text-gray-500"
                      >
                        Scope type
                      </label>
                      <select
                        id={`${fieldId}-scope`}
                        value={draft.scopeType}
                        onChange={(event) => {
                          const scopeType = event.target.value;
                          if (
                            scopeType === "unconstrained" ||
                            scopeType === "pack" ||
                            scopeType === "component"
                          ) {
                            updateDraft(draft.id, { scopeType, scopeRefs: "" });
                          }
                        }}
                        className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                      >
                        <option value="unconstrained">Unconstrained</option>
                        {canPackScope && (
                          <option value="pack">Pack scoped</option>
                        )}
                        {canComponentScope && (
                          <option value="component">
                            {componentScopeOptionLabel(draft.resource)}
                          </option>
                        )}
                      </select>
                    </div>
                    <ScopeInput
                      label={
                        draft.scopeType === "pack"
                          ? "Pack refs"
                          : componentScopeFieldLabel(draft.resource)
                      }
                      value={
                        draft.scopeType === "unconstrained"
                          ? ""
                          : draft.scopeRefs
                      }
                      placeholder={
                        draft.scopeType === "pack"
                          ? "core, slack"
                          : componentScopePlaceholder(draft.resource)
                      }
                      disabled={draft.scopeType === "unconstrained"}
                      onChange={(value) =>
                        updateDraft(draft.id, { scopeRefs: value })
                      }
                    />
                  </div>
                ) : (
                  <p className="text-sm text-gray-500">
                    This resource currently supports only unconstrained grants.
                  </p>
                )}
                {(showOwner ||
                  showOwnerType ||
                  showOwnerRefs ||
                  showVisibility ||
                  showExecutionScope ||
                  showEncrypted) && (
                  <div className="mt-5 grid gap-4 md:grid-cols-2">
                    {showOwner && (
                      <div>
                        <label
                          htmlFor={`${fieldId}-owner`}
                          className="block text-xs font-medium uppercase tracking-wide text-gray-500"
                        >
                          Owner
                        </label>
                        <select
                          id={`${fieldId}-owner`}
                          value={draft.owner}
                          onChange={(event) =>
                            updateDraft(draft.id, { owner: event.target.value })
                          }
                          className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                        >
                          <option value="">Any / not constrained</option>
                          <option value="self">Own resources</option>
                          <option value="any">Any owner</option>
                          <option value="none">No owner</option>
                        </select>
                      </div>
                    )}
                    {showOwnerType && (
                      <ScopeInput
                        label="Owner types"
                        value={draft.ownerTypes}
                        placeholder="identity, pack, action, sensor"
                        onChange={(value) =>
                          updateDraft(draft.id, { ownerTypes: value })
                        }
                      />
                    )}
                    {showOwnerRefs && (
                      <ScopeInput
                        label="Owner refs"
                        value={draft.ownerRefs}
                        placeholder="salesforce, my-sensor"
                        onChange={(value) =>
                          updateDraft(draft.id, { ownerRefs: value })
                        }
                      />
                    )}
                    {showExecutionScope && (
                      <div>
                        <label
                          htmlFor={`${fieldId}-execution`}
                          className="block text-xs font-medium uppercase tracking-wide text-gray-500"
                        >
                          Execution scope
                        </label>
                        <select
                          id={`${fieldId}-execution`}
                          value={draft.executionScope}
                          onChange={(event) =>
                            updateDraft(draft.id, {
                              executionScope: event.target.value,
                            })
                          }
                          className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                        >
                          <option value="">Any / not constrained</option>
                          <option value="self">Own executions</option>
                          <option value="descendants">Own + descendants</option>
                          <option value="any">All executions</option>
                        </select>
                      </div>
                    )}
                    {showEncrypted && (
                      <div>
                        <label
                          htmlFor={`${fieldId}-encrypted`}
                          className="block text-xs font-medium uppercase tracking-wide text-gray-500"
                        >
                          Key encryption scope
                        </label>
                        <select
                          id={`${fieldId}-encrypted`}
                          value={draft.encrypted}
                          onChange={(event) =>
                            updateDraft(draft.id, {
                              encrypted: event.target.value,
                            })
                          }
                          className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
                        >
                          <option value="">Any / not constrained</option>
                          <option value="true">Encrypted only</option>
                          <option value="false">Unencrypted only</option>
                        </select>
                      </div>
                    )}
                  </div>
                )}
                {showVisibility && (
                  <div className="mt-4">
                    <div className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                      Artifact visibility
                    </div>
                    <div className="mt-2 flex gap-2">
                      {["public", "private"].map((visibility) => (
                        <label
                          key={visibility}
                          className="inline-flex items-center gap-2 rounded-md border border-gray-200 bg-white px-3 py-1.5 text-sm text-gray-700"
                        >
                          <input
                            type="checkbox"
                            checked={draft.visibility.includes(visibility)}
                            onChange={() =>
                              updateDraft(draft.id, {
                                visibility: draft.visibility.includes(
                                  visibility,
                                )
                                  ? draft.visibility.filter(
                                      (entry) => entry !== visibility,
                                    )
                                  : [...draft.visibility, visibility],
                              })
                            }
                          />
                          {visibility}
                        </label>
                      ))}
                    </div>
                  </div>
                )}
                <div className="mt-4">
                  <label
                    htmlFor={`${fieldId}-attributes`}
                    className="block text-xs font-medium uppercase tracking-wide text-gray-500"
                  >
                    Identity attribute constraints JSON
                  </label>
                  <p className="mt-1 text-xs text-gray-500">
                    Optional ABAC filter. Every key/value here must exactly
                    match the requesting identity's stored attributes for the
                    grant to apply.
                  </p>
                  <textarea
                    id={`${fieldId}-attributes`}
                    value={draft.attributes}
                    onChange={(event) =>
                      updateDraft(draft.id, { attributes: event.target.value })
                    }
                    placeholder='{"environment": "prod"}'
                    rows={3}
                    className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 font-mono text-xs"
                  />
                </div>
              </div>
            </div>
          );
        })
      )}
      <button
        type="button"
        onClick={() => onChange([...drafts, newGrantDraft()])}
        className="inline-flex items-center gap-2 rounded-md border border-dashed border-blue-300 px-3 py-2 text-sm font-medium text-blue-700 hover:bg-blue-50"
      >
        <Plus className="h-4 w-4" />
        Add grant
      </button>
    </div>
  );
}

function ScopeInput({
  label,
  value,
  placeholder,
  disabled = false,
  onChange,
}: {
  label: string;
  value: string;
  placeholder: string;
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  const id = useId();
  return (
    <div>
      <label
        htmlFor={id}
        className="block text-xs font-medium uppercase tracking-wide text-gray-500"
      >
        {label}
      </label>
      <input
        id={id}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm disabled:cursor-not-allowed disabled:bg-gray-100 disabled:text-gray-400"
      />
      <p className="mt-1 text-xs text-gray-400">Comma-separated</p>
    </div>
  );
}
