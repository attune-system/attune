import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { AlertTriangle, Info, Plus, Trash2 } from "lucide-react";
import SearchableSelect from "@/components/common/SearchableSelect";
import { useAction, useActions } from "@/hooks/useActions";
import { usePacks } from "@/hooks/usePacks";
import {
  useCreatePolicy,
  usePolicies,
  useUpdatePolicy,
} from "@/hooks/usePolicies";
import { combineRefs, extractLocalRef, labelToRef } from "@/lib/format-utils";
import {
  PolicyMethod,
  PolicyScopeType,
  type ConcurrencyPolicyRequest,
  type CreatePolicyRequest,
  type PolicyResponse,
  type QuotaPolicyRequest,
  type RateLimitPolicyRequest,
  type UpdatePolicyRequest,
} from "@/api/policies";

type TimeUnit = "seconds" | "minutes" | "hours";

interface PolicyFormProps {
  initialData?: PolicyResponse;
  isEditing?: boolean;
}

const supportedQuotaTypes = [
  {
    value: "running_executions",
    label: "Running executions",
    description: "Maximum currently running executions that match this policy.",
  },
  {
    value: "executions_total",
    label: "Total executions",
    description: "Maximum historical executions that match this policy.",
  },
];

function getErrorMessage(error: unknown, fallback: string): string {
  const maybeApiError = error as { body?: { message?: string } };
  const maybeAxios = error as { response?: { data?: { message?: string } } };
  return (
    maybeApiError.body?.message ||
    maybeAxios.response?.data?.message ||
    (error instanceof Error ? error.message : fallback)
  );
}

function positiveInteger(label: string, value: string | number): number {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${label} must be a positive integer`);
  }
  return parsed;
}

function nonNegativeInteger(label: string, value: string | number): number {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative integer`);
  }
  return parsed;
}

function secondsToUnit(seconds: number): { value: number; unit: TimeUnit } {
  if (seconds % 3600 === 0) {
    return { value: seconds / 3600, unit: "hours" };
  }
  if (seconds % 60 === 0) {
    return { value: seconds / 60, unit: "minutes" };
  }
  return { value: seconds, unit: "seconds" };
}

function unitToSeconds(value: number, unit: TimeUnit): number {
  if (unit === "hours") return value * 3600;
  if (unit === "minutes") return value * 60;
  return value;
}

function splitTags(value: string): string[] {
  return value
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);
}

function getParamSuggestions(schema: unknown): string[] {
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) {
    return [];
  }
  return Object.keys(schema).sort();
}

function distinctNonEmpty(values: string[]): string[] {
  const seen = new Set<string>();
  const distinct: string[] = [];

  for (const value of values) {
    const trimmed = value.trim();
    if (!trimmed || seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    distinct.push(trimmed);
  }

  return distinct;
}

function Section({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-lg bg-white p-6 shadow">
      <div className="mb-5">
        <h2 className="text-lg font-semibold text-gray-900">{title}</h2>
        {description && (
          <p className="mt-1 text-sm text-gray-600">{description}</p>
        )}
      </div>
      {children}
    </section>
  );
}

function FieldError({ message }: { message?: string }) {
  if (!message) return null;
  return <p className="mt-1 text-sm text-red-600">{message}</p>;
}

function PolicyPrecedencePreview({
  scopeType,
  priority,
  packRef,
  actionRef,
}: {
  scopeType: PolicyScopeType;
  priority: number;
  packRef: string;
  actionRef: string;
}) {
  const { data } = usePolicies({
    pageSize: 100,
    packRef: packRef || undefined,
    actionRef: actionRef || undefined,
    enabled: true,
  });
  const matchingPolicies = data?.items ?? [];
  const sameScope = matchingPolicies
    .filter((policy) => policy.scope.type === scopeType)
    .filter((policy) =>
      scopeType === PolicyScopeType.ACTION
        ? policy.scope.action_ref === actionRef
        : scopeType === PolicyScopeType.PACK
          ? policy.scope.pack_ref === packRef
          : true,
    )
    .sort((a, b) => b.priority - a.priority);
  const strongest = sameScope[0];
  const scopeLabel =
    scopeType === PolicyScopeType.ACTION
      ? `action ${actionRef || "(select an action)"}`
      : scopeType === PolicyScopeType.PACK
        ? `pack ${packRef || "(select a pack)"}`
        : "all executions";

  return (
    <div className="rounded-lg border border-blue-200 bg-blue-50 p-4 text-sm text-blue-900">
      <div className="flex gap-2">
        <Info className="mt-0.5 h-4 w-4 flex-shrink-0" />
        <div>
          <div className="font-medium">Effective policy preview</div>
          <p className="mt-1">
            This policy targets {scopeLabel}. Attune resolves one effective
            policy: action-scoped policies override pack-scoped policies, which
            override global policies. Within the same scope, higher priority
            wins.
          </p>
          {strongest && (
            <p className="mt-2">
              Current strongest same-scope policy is{" "}
              <span className="font-mono">{strongest.ref}</span> with priority{" "}
              {strongest.priority}. The new priority is {priority}.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

export default function PolicyForm({
  initialData,
  isEditing = false,
}: PolicyFormProps) {
  const navigate = useNavigate();
  const createPolicy = useCreatePolicy();
  const updatePolicy = useUpdatePolicy();
  const { data: packsData } = usePacks({ pageSize: 1000 });
  const { data: actionsData } = useActions({ pageSize: 1000 });

  const packs = packsData?.items ?? [];
  const actions = actionsData?.items ?? [];
  const initialScope = initialData?.scope;
  const [name, setName] = useState(() => initialData?.name ?? "");
  const [description, setDescription] = useState(
    () => initialData?.description ?? "",
  );
  const [enabled, setEnabled] = useState(() => initialData?.enabled ?? true);
  const [priority, setPriority] = useState(() => initialData?.priority ?? 0);
  const [scopeType, setScopeType] = useState<PolicyScopeType>(
    () => initialScope?.type ?? PolicyScopeType.ACTION,
  );
  const [packRef, setPackRef] = useState(() => initialScope?.pack_ref ?? "");
  const [actionRef, setActionRef] = useState(
    () => initialScope?.action_ref ?? "",
  );
  const [localRef, setLocalRef] = useState(() =>
    initialData?.ref
      ? extractLocalRef(initialData.ref, initialScope?.pack_ref ?? undefined)
      : "",
  );
  const [tagsInput, setTagsInput] = useState(() =>
    (initialData?.tags ?? []).join(", "),
  );
  const [concurrencyEnabled, setConcurrencyEnabled] = useState(
    () => !!initialData?.concurrency,
  );
  const [concurrencyLimit, setConcurrencyLimit] = useState(
    () => initialData?.concurrency?.limit ?? 1,
  );
  const [concurrencyMethod, setConcurrencyMethod] = useState<PolicyMethod>(
    () => initialData?.concurrency?.method ?? PolicyMethod.ENQUEUE,
  );
  const [groupingPaths, setGroupingPaths] = useState<string[]>(
    () => initialData?.concurrency?.parameters ?? [],
  );
  const [rateLimitEnabled, setRateLimitEnabled] = useState(
    () => !!initialData?.rate_limit,
  );
  const initialWindow = secondsToUnit(
    initialData?.rate_limit?.window_seconds ?? 60,
  );
  const [rateLimitMax, setRateLimitMax] = useState(
    () => initialData?.rate_limit?.max_executions ?? 10,
  );
  const [rateLimitWindow, setRateLimitWindow] = useState(initialWindow.value);
  const [rateLimitUnit, setRateLimitUnit] = useState<TimeUnit>(
    initialWindow.unit,
  );
  const [quotas, setQuotas] = useState<QuotaPolicyRequest[]>(
    () => initialData?.quotas ?? [],
  );
  const [formError, setFormError] = useState<string | null>(null);

  const { data: selectedActionData } = useAction(actionRef);
  const paramSuggestions = useMemo(
    () => getParamSuggestions(selectedActionData?.data.param_schema),
    [selectedActionData?.data.param_schema],
  );
  const distinctGroupingPaths = useMemo(
    () => distinctNonEmpty(groupingPaths),
    [groupingPaths],
  );
  const filteredActions = packRef
    ? actions.filter((action) => action.pack_ref === packRef)
    : actions;
  const effectiveRef =
    scopeType === PolicyScopeType.GLOBAL
      ? localRef
      : packRef && localRef
        ? combineRefs(packRef, localRef)
        : localRef;
  const isSaving = createPolicy.isPending || updatePolicy.isPending;

  const buildConcurrency = (): ConcurrencyPolicyRequest | null => {
    if (!concurrencyEnabled) return null;
    return {
      limit: positiveInteger("Concurrency limit", concurrencyLimit),
      method: concurrencyMethod,
      parameters: distinctNonEmpty(groupingPaths),
    };
  };

  const buildRateLimit = (): RateLimitPolicyRequest | null => {
    if (!rateLimitEnabled) return null;
    const maxExecutions = positiveInteger("Rate limit maximum", rateLimitMax);
    const windowValue = positiveInteger("Rate limit window", rateLimitWindow);
    return {
      max_executions: maxExecutions,
      window_seconds: unitToSeconds(windowValue, rateLimitUnit),
    };
  };

  const buildScope = () => {
    if (scopeType === PolicyScopeType.GLOBAL) {
      return { type: scopeType };
    }
    if (scopeType === PolicyScopeType.PACK) {
      if (!packRef) throw new Error("Pack scope requires a pack");
      return { type: scopeType, pack_ref: packRef };
    }
    if (!actionRef) throw new Error("Action scope requires an action");
    return { type: scopeType, action_ref: actionRef };
  };

  const validateQuotas = (): QuotaPolicyRequest[] =>
    quotas.map((quota, index) => {
      if (!quota.quota_type) {
        throw new Error(`Quota ${index + 1} requires a quota type`);
      }
      return {
        quota_type: quota.quota_type,
        limit: positiveInteger(`Quota ${index + 1} limit`, quota.limit),
      };
    });

  const addGroupingPath = (path = "") => {
    setGroupingPaths((current) => {
      const nextPath = path.trim();
      if (nextPath && current.map((value) => value.trim()).includes(nextPath)) {
        return current;
      }
      return [...current, nextPath];
    });
  };

  const updateGroupingPath = (index: number, value: string) => {
    setGroupingPaths((current) =>
      current.map((path, pathIndex) => (pathIndex === index ? value : path)),
    );
  };

  const removeGroupingPath = (index: number) => {
    setGroupingPaths((current) =>
      current.filter((_, pathIndex) => pathIndex !== index),
    );
  };

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    setFormError(null);

    try {
      if (!isEditing && !effectiveRef.trim()) {
        throw new Error("Policy ref is required");
      }
      if (!name.trim()) {
        throw new Error("Policy name is required");
      }
      if (!concurrencyEnabled && !rateLimitEnabled && quotas.length === 0) {
        throw new Error("Enable at least one policy feature");
      }

      const common = {
        name: name.trim(),
        description: description.trim() || null,
        enabled,
        priority: nonNegativeInteger("Priority", priority),
        concurrency: buildConcurrency(),
        rate_limit: buildRateLimit(),
        quotas: validateQuotas(),
        tags: splitTags(tagsInput),
      };

      if (isEditing && initialData) {
        const payload: UpdatePolicyRequest = common;
        const response = await updatePolicy.mutateAsync({
          ref: initialData.ref,
          data: payload,
        });
        navigate(`/policies/${encodeURIComponent(response.data.ref)}`);
      } else {
        const payload: CreatePolicyRequest = {
          ref: effectiveRef.trim(),
          scope: buildScope(),
          ...common,
        };
        const response = await createPolicy.mutateAsync(payload);
        navigate(`/policies/${encodeURIComponent(response.data.ref)}`);
      }
    } catch (error) {
      setFormError(getErrorMessage(error, "Failed to save policy"));
    }
  };

  return (
    <form onSubmit={handleSubmit} className="space-y-6">
      {formError && (
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {formError}
        </div>
      )}

      <Section
        title="Policy identity"
        description="Name the policy, choose its operational state, and set same-scope precedence."
      >
        <div className="grid gap-4 md:grid-cols-2">
          <div>
            <label className="block text-sm font-medium text-gray-700">
              Name
            </label>
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              onBlur={() => {
                if (!isEditing && !localRef.trim()) {
                  setLocalRef(labelToRef(name));
                }
              }}
              className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Limit production deployments"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700">
              Ref
            </label>
            <input
              value={effectiveRef}
              disabled={isEditing}
              onChange={(event) =>
                setLocalRef(extractLocalRef(event.target.value))
              }
              className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm disabled:bg-gray-100"
              placeholder="pack.limit_production_deployments"
            />
            {!isEditing && scopeType !== PolicyScopeType.GLOBAL && (
              <p className="mt-1 text-xs text-gray-500">
                Pack and action policies are stored as pack-qualified refs.
              </p>
            )}
          </div>
          <div className="md:col-span-2">
            <label className="block text-sm font-medium text-gray-700">
              Description
            </label>
            <textarea
              value={description}
              onChange={(event) => setDescription(event.target.value)}
              className="mt-1 min-h-20 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="Explain when this policy applies and how violations are handled."
            />
          </div>
          <label className="flex items-center gap-2 text-sm text-gray-700">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(event) => setEnabled(event.target.checked)}
              className="rounded border-gray-300"
            />
            Enabled
          </label>
          <div>
            <label className="block text-sm font-medium text-gray-700">
              Priority
            </label>
            <input
              type="number"
              min={0}
              value={priority}
              onChange={(event) => setPriority(Number(event.target.value))}
              className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
            />
          </div>
          <div className="md:col-span-2">
            <label className="block text-sm font-medium text-gray-700">
              Tags
            </label>
            <input
              value={tagsInput}
              onChange={(event) => setTagsInput(event.target.value)}
              className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              placeholder="production, deploy, customer-facing"
            />
            <p className="mt-1 text-xs text-gray-500">
              Separate tags with commas.
            </p>
          </div>
        </div>
      </Section>

      <Section
        title="Scope"
        description="Choose where the policy applies. Action scope is most specific."
      >
        <div className="grid gap-4 md:grid-cols-3">
          {Object.values(PolicyScopeType).map((type) => (
            <button
              key={type}
              type="button"
              disabled={isEditing}
              onClick={() => {
                setScopeType(type);
                if (type === PolicyScopeType.GLOBAL) {
                  setPackRef("");
                  setActionRef("");
                }
              }}
              className={`rounded-lg border p-4 text-left transition-colors disabled:cursor-not-allowed ${
                scopeType === type
                  ? "border-blue-500 bg-blue-50 text-blue-900"
                  : "border-gray-200 bg-white hover:border-gray-300"
              }`}
            >
              <div className="font-medium capitalize">{type}</div>
              <div className="mt-1 text-sm text-gray-600">
                {type === PolicyScopeType.ACTION
                  ? "Only one action"
                  : type === PolicyScopeType.PACK
                    ? "All actions in a pack"
                    : "Fallback for all executions"}
              </div>
            </button>
          ))}
        </div>
        {scopeType !== PolicyScopeType.GLOBAL && (
          <div className="mt-4 grid gap-4 md:grid-cols-2">
            <div>
              <label className="block text-sm font-medium text-gray-700">
                Pack
              </label>
              <SearchableSelect
                options={packs.map((pack) => ({
                  value: pack.ref,
                  label: `${pack.ref} - ${pack.label}`,
                }))}
                value={packRef}
                disabled={isEditing}
                onChange={(value) => {
                  const next = String(value);
                  setPackRef(next);
                  if (actionRef) {
                    const currentAction = actions.find(
                      (action) => action.ref === actionRef,
                    );
                    if (currentAction?.pack_ref !== next) {
                      setActionRef("");
                    }
                  }
                }}
                placeholder="Select a pack"
              />
            </div>
            {scopeType === PolicyScopeType.ACTION && (
              <div>
                <label className="block text-sm font-medium text-gray-700">
                  Action
                </label>
                <SearchableSelect
                  options={filteredActions.map((action) => ({
                    value: action.ref,
                    label: `${action.ref} - ${action.label}`,
                  }))}
                  value={actionRef}
                  disabled={isEditing}
                  onChange={(value) => {
                    const next = String(value);
                    setActionRef(next);
                    const action = actions.find(
                      (candidate) => candidate.ref === next,
                    );
                    if (action) setPackRef(action.pack_ref);
                  }}
                  placeholder="Select an action"
                />
              </div>
            )}
          </div>
        )}
        <div className="mt-4">
          <PolicyPrecedencePreview
            scopeType={scopeType}
            priority={Number(priority) || 0}
            packRef={packRef}
            actionRef={actionRef}
          />
        </div>
      </Section>

      <Section
        title="Concurrency"
        description="Limit simultaneous matching executions and choose what happens when the limit is reached."
      >
        <label className="mb-4 flex items-center gap-2 text-sm text-gray-700">
          <input
            type="checkbox"
            checked={concurrencyEnabled}
            onChange={(event) => setConcurrencyEnabled(event.target.checked)}
            className="rounded border-gray-300"
          />
          Enable concurrency limit
        </label>
        {concurrencyEnabled && (
          <div className="grid gap-4 md:grid-cols-2">
            <div>
              <label className="block text-sm font-medium text-gray-700">
                Concurrent execution limit
              </label>
              <input
                type="number"
                min={1}
                value={concurrencyLimit}
                onChange={(event) =>
                  setConcurrencyLimit(Number(event.target.value))
                }
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700">
                When the limit is reached
              </label>
              <div className="mt-1 grid grid-cols-2 gap-2">
                {Object.values(PolicyMethod).map((method) => (
                  <button
                    key={method}
                    type="button"
                    onClick={() => setConcurrencyMethod(method)}
                    className={`rounded-lg border px-3 py-2 text-sm capitalize ${
                      concurrencyMethod === method
                        ? "border-blue-500 bg-blue-50 text-blue-900"
                        : "border-gray-200 bg-white hover:border-gray-300"
                    }`}
                  >
                    {method}
                  </button>
                ))}
              </div>
            </div>
            <div className="md:col-span-2">
              <label className="block text-sm font-medium text-gray-700">
                Group by parameter paths
              </label>
              <div className="mt-1 space-y-2">
                {groupingPaths.length === 0 ? (
                  <div className="rounded-lg border border-dashed border-gray-300 bg-gray-50 px-4 py-3 text-sm text-gray-600">
                    No grouping paths configured. The concurrency limit applies
                    as one shared pool.
                  </div>
                ) : (
                  groupingPaths.map((path, index) => {
                    const trimmedPath = path.trim();
                    const isDuplicate =
                      trimmedPath.length > 0 &&
                      groupingPaths
                        .map((candidate) => candidate.trim())
                        .filter((candidate) => candidate === trimmedPath)
                        .length > 1;
                    return (
                      <div key={index} className="flex items-start gap-2">
                        <div className="flex-1">
                          <input
                            value={path}
                            onChange={(event) =>
                              updateGroupingPath(index, event.target.value)
                            }
                            onBlur={() =>
                              setGroupingPaths((current) =>
                                current.map((candidate, candidateIndex) =>
                                  candidateIndex === index
                                    ? candidate.trim()
                                    : candidate,
                                ),
                              )
                            }
                            className={`w-full rounded-lg border px-3 py-2 font-mono text-sm ${
                              isDuplicate
                                ? "border-amber-400 bg-amber-50"
                                : "border-gray-300"
                            }`}
                            placeholder="customer_id"
                          />
                          {isDuplicate && (
                            <p className="mt-1 text-xs text-amber-700">
                              Duplicate paths are collapsed when saved.
                            </p>
                          )}
                        </div>
                        <button
                          type="button"
                          onClick={() => removeGroupingPath(index)}
                          className="inline-flex h-10 items-center justify-center rounded-lg border border-red-200 px-3 text-red-700 hover:bg-red-50"
                          aria-label={`Remove grouping path ${index + 1}`}
                        >
                          <Trash2 className="h-4 w-4" />
                        </button>
                      </div>
                    );
                  })
                )}
              </div>
              <p className="mt-1 text-xs text-gray-500">
                Add one parameter path per input. Leave empty for a single
                shared limit.
              </p>
              <button
                type="button"
                onClick={() => addGroupingPath()}
                className="mt-3 inline-flex items-center gap-2 rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50"
              >
                <Plus className="h-4 w-4" />
                Add grouping path
              </button>
              {paramSuggestions.length > 0 && (
                <div className="mt-3 flex flex-wrap gap-2">
                  {paramSuggestions.map((path) => (
                    <button
                      key={path}
                      type="button"
                      onClick={() => addGroupingPath(path)}
                      disabled={distinctGroupingPaths.includes(path)}
                      className="rounded-full border border-gray-300 px-3 py-1 text-xs text-gray-700 hover:bg-gray-50 disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      Add {path}
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>
        )}
      </Section>

      <Section
        title="Rate limit"
        description="Limit the number of matching executions during a rolling time window."
      >
        <label className="mb-4 flex items-center gap-2 text-sm text-gray-700">
          <input
            type="checkbox"
            checked={rateLimitEnabled}
            onChange={(event) => setRateLimitEnabled(event.target.checked)}
            className="rounded border-gray-300"
          />
          Enable rate limit
        </label>
        {rateLimitEnabled && (
          <div className="grid gap-4 md:grid-cols-3">
            <div>
              <label className="block text-sm font-medium text-gray-700">
                Max executions
              </label>
              <input
                type="number"
                min={1}
                value={rateLimitMax}
                onChange={(event) =>
                  setRateLimitMax(Number(event.target.value))
                }
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700">
                Window
              </label>
              <input
                type="number"
                min={1}
                value={rateLimitWindow}
                onChange={(event) =>
                  setRateLimitWindow(Number(event.target.value))
                }
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700">
                Unit
              </label>
              <select
                value={rateLimitUnit}
                onChange={(event) =>
                  setRateLimitUnit(event.target.value as TimeUnit)
                }
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
              >
                <option value="seconds">Seconds</option>
                <option value="minutes">Minutes</option>
                <option value="hours">Hours</option>
              </select>
            </div>
            <div className="md:col-span-3 rounded-lg bg-gray-50 px-4 py-3 text-sm text-gray-700">
              Preview: {rateLimitMax || 0} executions per {rateLimitWindow || 0}{" "}
              {rateLimitUnit}.
            </div>
          </div>
        )}
      </Section>

      <Section
        title="Quotas"
        description="Add quota checks for supported execution counters."
      >
        <div className="space-y-3">
          {quotas.map((quota, index) => {
            const quotaInfo = supportedQuotaTypes.find(
              (item) => item.value === quota.quota_type,
            );
            return (
              <div
                key={`${quota.quota_type}-${index}`}
                className="rounded-lg border border-gray-200 p-4"
              >
                <div className="grid gap-4 md:grid-cols-[1fr_160px_auto]">
                  <div>
                    <label className="block text-sm font-medium text-gray-700">
                      Quota type
                    </label>
                    <select
                      value={quota.quota_type}
                      onChange={(event) =>
                        setQuotas((current) =>
                          current.map((item, itemIndex) =>
                            itemIndex === index
                              ? { ...item, quota_type: event.target.value }
                              : item,
                          ),
                        )
                      }
                      className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                    >
                      <option value="">Select quota type</option>
                      {supportedQuotaTypes.map((item) => (
                        <option key={item.value} value={item.value}>
                          {item.label}
                        </option>
                      ))}
                    </select>
                    {quotaInfo && (
                      <p className="mt-1 text-xs text-gray-500">
                        {quotaInfo.description}
                      </p>
                    )}
                  </div>
                  <div>
                    <label className="block text-sm font-medium text-gray-700">
                      Limit
                    </label>
                    <input
                      type="number"
                      min={1}
                      value={quota.limit}
                      onChange={(event) =>
                        setQuotas((current) =>
                          current.map((item, itemIndex) =>
                            itemIndex === index
                              ? { ...item, limit: Number(event.target.value) }
                              : item,
                          ),
                        )
                      }
                      className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                    />
                  </div>
                  <button
                    type="button"
                    onClick={() =>
                      setQuotas((current) =>
                        current.filter((_, itemIndex) => itemIndex !== index),
                      )
                    }
                    className="mt-6 inline-flex h-10 items-center justify-center rounded-lg border border-red-200 px-3 text-red-700 hover:bg-red-50"
                    aria-label="Remove quota"
                  >
                    <Trash2 className="h-4 w-4" />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
        <button
          type="button"
          onClick={() =>
            setQuotas((current) => [
              ...current,
              { quota_type: "running_executions", limit: 1 },
            ])
          }
          className="mt-4 inline-flex items-center gap-2 rounded-lg border border-gray-300 px-4 py-2 text-sm text-gray-700 hover:bg-gray-50"
        >
          <Plus className="h-4 w-4" />
          Add quota
        </button>
      </Section>

      {!concurrencyEnabled && !rateLimitEnabled && quotas.length === 0 && (
        <div className="flex gap-2 rounded-lg border border-amber-200 bg-amber-50 p-4 text-sm text-amber-900">
          <AlertTriangle className="mt-0.5 h-4 w-4 flex-shrink-0" />
          Enable at least one policy feature before saving.
        </div>
      )}

      <div className="flex justify-end gap-3">
        <button
          type="button"
          onClick={() => navigate("/policies")}
          className="rounded-lg border border-gray-300 px-4 py-2 text-gray-700 hover:bg-gray-50"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={isSaving}
          className="rounded-lg bg-blue-600 px-4 py-2 text-white hover:bg-blue-700 disabled:opacity-60"
        >
          {isSaving
            ? "Saving..."
            : isEditing
              ? "Update Policy"
              : "Create Policy"}
        </button>
      </div>
      <FieldError message={formError ?? undefined} />
    </form>
  );
}
