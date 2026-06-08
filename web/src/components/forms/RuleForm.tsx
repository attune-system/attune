import { useState, useEffect, useMemo, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { usePacks } from "@/hooks/usePacks";
import { useTriggers, useTrigger } from "@/hooks/useTriggers";
import { useActions, useAction } from "@/hooks/useActions";
import { useCreateRule, useUpdateRule } from "@/hooks/useRules";
import ParamSchemaForm, {
  validateParamSchema,
  type ParamSchema,
} from "@/components/common/ParamSchemaForm";
import SearchableSelect from "@/components/common/SearchableSelect";
import RuleMatchConditionsEditor from "@/components/forms/RuleMatchConditionsEditor";
import type {
  RuleResponse,
  ActionSummary,
  TriggerResponse,
  ActionResponse,
} from "@/types/api";
import type { CreateRuleRequest, UpdateRuleRequest } from "@/api";
import { labelToRef, extractLocalRef, combineRefs } from "@/lib/format-utils";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type JsonValue = any;
type PermissionOverrideMode = "inherit" | "none" | "custom";

interface RuleFormProps {
  rule?: RuleResponse;
  onSuccess?: () => void;
  onCancel?: () => void;
}

export default function RuleForm({ rule, onSuccess, onCancel }: RuleFormProps) {
  const navigate = useNavigate();
  const isEditing = !!rule;

  // Form state
  const [packId, setPackId] = useState<number>(rule?.pack || 0);
  const [localRef, setLocalRef] = useState(
    rule?.ref ? extractLocalRef(rule.ref) : "",
  );
  const [label, setLabel] = useState(rule?.label || "");
  const [description, setDescription] = useState(rule?.description || "");
  const [triggerId, setTriggerId] = useState<number>(rule?.trigger || 0);
  const [actionId, setActionId] = useState<number>(rule?.action || 0);
  const [conditions, setConditions] = useState<JsonValue | undefined>(() => {
    if (!rule?.conditions) {
      return undefined;
    }

    if (
      typeof rule.conditions === "object" &&
      !Array.isArray(rule.conditions) &&
      Object.keys(rule.conditions).length === 0
    ) {
      return undefined;
    }

    return rule.conditions;
  });
  const [triggerParameters, setTriggerParameters] = useState<
    Record<string, JsonValue>
  >(rule?.trigger_params || {});
  const [actionParameters, setActionParameters] = useState<
    Record<string, JsonValue>
  >(rule?.action_params || {});
  const initialPermissionSetRefs = rule?.permission_set_refs ?? null;
  const [permissionMode, setPermissionMode] = useState<PermissionOverrideMode>(
    initialPermissionSetRefs === null
      ? "inherit"
      : initialPermissionSetRefs.length === 0
        ? "none"
        : "custom",
  );
  const [permissionSetRefsInput, setPermissionSetRefsInput] = useState(
    () => initialPermissionSetRefs?.join(", ") ?? "",
  );
  const [enabled, setEnabled] = useState(rule?.enabled ?? true);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [triggerParamErrors, setTriggerParamErrors] = useState<
    Record<string, string>
  >({});
  const [actionParamErrors, setActionParamErrors] = useState<
    Record<string, string>
  >({});
  const [conditionsError, setConditionsError] = useState<string | undefined>();
  const previousTriggerIdRef = useRef(triggerId);
  const previousActionIdRef = useRef(actionId);

  // Data fetching
  const { data: packsData } = usePacks({ pageSize: 1000 });
  const packs = useMemo(() => packsData?.items || [], [packsData?.items]);

  const selectedPack = packs.find((p) => p.id === packId);

  // Fetch ALL triggers and actions from all packs, not just the selected pack
  // This allows rules in ad-hoc packs to reference triggers/actions from other packs
  const { data: triggersData } = useTriggers({ pageSize: 1000 });
  const { data: actionsData } = useActions({
    pageSize: 1000,
    referencingPackRef: selectedPack?.ref,
  });

  const triggers = triggersData?.items || [];
  const actions = actionsData?.items || [];

  // Get selected trigger and action refs for detail fetching
  const selectedTriggerSummary = triggers.find((t) => t.id === triggerId);
  const selectedActionSummary = actions.find(
    (a: ActionSummary) => a.id === actionId,
  );

  // Fetch full trigger details (including param_schema) when a trigger is selected
  const { data: triggerDetailsData } = useTrigger(
    selectedTriggerSummary?.ref || "",
  );
  const selectedTrigger = triggerDetailsData?.data;

  // Fetch full action details (including param_schema) when an action is selected
  const { data: actionDetailsData } = useAction(
    selectedActionSummary?.ref || "",
  );
  const selectedAction = actionDetailsData?.data;

  // Extract param schemas from full details
  const triggerParamSchema: ParamSchema =
    ((selectedTrigger as TriggerResponse | undefined)
      ?.param_schema as ParamSchema) || {};
  const actionParamSchema: ParamSchema =
    ((selectedAction as ActionResponse | undefined)
      ?.param_schema as ParamSchema) || {};

  // Mutations
  const createRule = useCreateRule();
  const updateRule = useUpdateRule();

  // Reset triggers, actions, and parameters when pack changes
  /* eslint-disable react-hooks/set-state-in-effect -- intentional dependent-state reset */
  useEffect(() => {
    if (!isEditing) {
      setTriggerId(0);
      setActionId(0);
      setTriggerParameters({});
      setActionParameters({});
    }
  }, [packId, isEditing]);
  /* eslint-enable react-hooks/set-state-in-effect */

  // Reset trigger parameters when trigger changes
  /* eslint-disable react-hooks/set-state-in-effect -- intentional dependent-state reset */
  useEffect(() => {
    if (previousTriggerIdRef.current !== triggerId) {
      setTriggerParameters({});
      setTriggerParamErrors({});
    }
    previousTriggerIdRef.current = triggerId;
  }, [triggerId, isEditing]);
  /* eslint-enable react-hooks/set-state-in-effect */

  // Reset action parameters when action changes
  /* eslint-disable react-hooks/set-state-in-effect -- intentional dependent-state reset */
  useEffect(() => {
    if (previousActionIdRef.current !== actionId) {
      setActionParameters({});
      setActionParamErrors({});
    }
    previousActionIdRef.current = actionId;
  }, [actionId, isEditing]);
  /* eslint-enable react-hooks/set-state-in-effect */

  const validateForm = (): boolean => {
    const newErrors: Record<string, string> = {};

    if (!localRef.trim()) {
      newErrors.ref = "Reference is required";
    }

    if (!label.trim()) {
      newErrors.label = "Label is required";
    }

    if (!packId) {
      newErrors.pack = "Pack is required";
    }

    if (!triggerId) {
      newErrors.trigger = "Trigger is required";
    }

    if (!actionId) {
      newErrors.action = "Action is required";
    }

    if (conditionsError) {
      newErrors.conditions = conditionsError;
    }

    // Validate trigger parameters (allow templates in rule context)
    const triggerErrors = validateParamSchema(
      triggerParamSchema,
      triggerParameters,
      true,
    );
    setTriggerParamErrors(triggerErrors);

    // Validate action parameters (allow templates in rule context)
    const actionErrors = validateParamSchema(
      actionParamSchema,
      actionParameters,
      true,
    );
    setActionParamErrors(actionErrors);

    setErrors(newErrors);

    return (
      Object.keys(newErrors).length === 0 &&
      Object.keys(triggerErrors).length === 0 &&
      Object.keys(actionErrors).length === 0
    );
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!validateForm()) {
      return;
    }

    // Get the selected pack, trigger, and action
    const selectedPackData = packs.find((p) => p.id === packId);

    // Combine pack ref and local ref to create full ref
    const fullRef = combineRefs(selectedPackData?.ref || "", localRef.trim());

    const formData: Record<string, JsonValue> & Partial<CreateRuleRequest> = {
      pack_ref: selectedPackData?.ref || "",
      ref: fullRef,
      label: label.trim(),
      trigger_ref: selectedTrigger?.ref || "",
      action_ref: selectedAction?.ref || "",
      enabled,
    };

    if (description.trim()) {
      formData.description = description.trim();
    }

    // Only add optional fields if they have values
    if (conditions !== undefined) {
      formData.conditions = conditions;
    }

    if (isEditing || Object.keys(triggerParameters).length > 0) {
      formData.trigger_params = triggerParameters;
    }

    if (isEditing || Object.keys(actionParameters).length > 0) {
      formData.action_params = actionParameters;
    }

    if (permissionMode === "custom") {
      formData.permission_set_refs = permissionSetRefsInput
        .split(",")
        .map((ref) => ref.trim())
        .filter(Boolean);
    } else if (permissionMode === "none") {
      formData.permission_set_refs = [];
    } else if (isEditing) {
      formData.permission_set_refs = null;
    }

    try {
      if (isEditing && rule) {
        await updateRule.mutateAsync({
          ref: rule.ref,
          data: formData as unknown as UpdateRuleRequest,
        });
      } else {
        const newRuleResponse = await createRule.mutateAsync(
          formData as unknown as CreateRuleRequest,
        );
        if (!onSuccess) {
          navigate(`/rules/${newRuleResponse.data.ref}`);
        }
      }

      if (onSuccess) {
        onSuccess();
      }
    } catch (err) {
      console.error("Failed to save rule:", err);
      setErrors({
        submit: err instanceof Error ? err.message : "Failed to save rule",
      });
    }
  };

  const handleCancel = () => {
    if (onCancel) {
      onCancel();
    } else {
      navigate("/rules");
    }
  };

  return (
    <form onSubmit={handleSubmit} className="space-y-6">
      {errors.submit && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-4">
          <p className="text-sm text-red-600">{errors.submit}</p>
        </div>
      )}

      {/* Basic Information */}
      <div className="bg-white rounded-lg shadow p-5 lg:p-6">
        <h3 className="text-lg font-semibold text-gray-900">
          Basic Information
        </h3>

        <div className="mt-4 grid grid-cols-1 gap-4 lg:grid-cols-12">
          {/* Pack Selection */}
          <div className="lg:col-span-4">
            <label
              htmlFor="pack"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Pack <span className="text-red-500">*</span>
            </label>
            <SearchableSelect
              id="pack"
              value={packId}
              onChange={(v) => setPackId(Number(v))}
              options={packs.map((pack) => ({
                value: pack.id,
                label: `${pack.label} (${pack.version})`,
              }))}
              placeholder="Select a pack..."
              disabled={isEditing}
              error={!!errors.pack}
            />
            {errors.pack && (
              <p className="mt-1 text-sm text-red-600">{errors.pack}</p>
            )}
          </div>

          {/* Label */}
          <div className="lg:col-span-8">
            <label
              htmlFor="label"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Label <span className="text-red-500">*</span>
            </label>
            <input
              type="text"
              id="label"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              onBlur={() => {
                // Auto-populate localRef from label if localRef is empty and not editing
                if (!isEditing && !localRef.trim() && label.trim()) {
                  setLocalRef(labelToRef(label));
                }
              }}
              placeholder="e.g., Notify on Error"
              className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                errors.label ? "border-red-500" : "border-gray-300"
              }`}
            />
            {errors.label && (
              <p className="mt-1 text-sm text-red-600">{errors.label}</p>
            )}
          </div>

          {/* Reference */}
          <div className="lg:col-span-7">
            <label
              htmlFor="ref"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Reference <span className="text-red-500">*</span>
            </label>
            <div className="flex flex-col gap-3 xl:flex-row xl:items-center">
              <div className="input-with-prefix flex-1">
                <span className={`prefix ${errors.ref ? "error" : ""}`}>
                  {selectedPack?.ref || "pack"}.
                </span>
                <input
                  type="text"
                  id="ref"
                  value={localRef}
                  onChange={(e) => setLocalRef(e.target.value)}
                  placeholder="e.g., notify_on_error"
                  disabled={isEditing}
                  className={errors.ref ? "error" : ""}
                />
              </div>
              <label
                htmlFor="enabled"
                className="flex items-center gap-2 whitespace-nowrap rounded-lg border border-gray-200 px-3 py-2.5 text-sm text-gray-700"
              >
                <input
                  type="checkbox"
                  id="enabled"
                  checked={enabled}
                  onChange={(e) => setEnabled(e.target.checked)}
                  className="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                />
                Enable immediately
              </label>
            </div>
            {errors.ref && (
              <p className="mt-1 text-sm text-red-600">{errors.ref}</p>
            )}
          </div>

          {/* Description */}
          <div className="lg:col-span-12">
            <label
              htmlFor="description"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Description
            </label>
            <textarea
              id="description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Describe what this rule does..."
              rows={2}
              className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
                errors.description ? "border-red-500" : "border-gray-300"
              }`}
            />
            {errors.description && (
              <p className="mt-1 text-sm text-red-600">{errors.description}</p>
            )}
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-6 xl:grid-cols-2">
        {/* Trigger Configuration */}
        <div className="bg-white rounded-lg shadow p-5 lg:p-6 space-y-4">
          <h3 className="text-lg font-semibold text-gray-900">
            Trigger Configuration
          </h3>

          {!triggers || triggers.length === 0 ? (
            <p className="text-sm text-gray-500">
              No triggers available in the system
            </p>
          ) : (
            <>
              {/* Trigger Selection */}
              <div>
                <label
                  htmlFor="trigger"
                  className="block text-sm font-medium text-gray-700 mb-1"
                >
                  Trigger <span className="text-red-500">*</span>
                </label>
                <SearchableSelect
                  id="trigger"
                  value={triggerId}
                  onChange={(v) => setTriggerId(Number(v))}
                  options={triggers.map((trigger) => ({
                    value: trigger.id,
                    label: `${trigger.ref} - ${trigger.label}`,
                  }))}
                  placeholder="Select a trigger..."
                  error={!!errors.trigger}
                />
                {errors.trigger && (
                  <p className="mt-1 text-sm text-red-600">{errors.trigger}</p>
                )}
              </div>

              {/* Trigger Parameters - Dynamic Form */}
              {selectedTrigger && (
                <div>
                  <h4 className="text-sm font-medium text-gray-700 mb-3">
                    Trigger Parameters
                  </h4>
                  <ParamSchemaForm
                    schema={triggerParamSchema}
                    values={triggerParameters}
                    onChange={setTriggerParameters}
                    errors={triggerParamErrors}
                    allowTemplates
                  />
                </div>
              )}

              <RuleMatchConditionsEditor
                value={conditions}
                onChange={setConditions}
                error={errors.conditions}
                onErrorChange={setConditionsError}
              />
            </>
          )}
        </div>

        {/* Action Configuration */}
        <div className="bg-white rounded-lg shadow p-5 lg:p-6 space-y-4">
          <h3 className="text-lg font-semibold text-gray-900">
            Action Configuration
          </h3>

          {!actions || actions.length === 0 ? (
            <p className="text-sm text-gray-500">
              No actions available in the system
            </p>
          ) : (
            <>
              {/* Action Selection */}
              <div>
                <label
                  htmlFor="action"
                  className="block text-sm font-medium text-gray-700 mb-1"
                >
                  Action <span className="text-red-500">*</span>
                </label>
                <SearchableSelect
                  id="action"
                  value={actionId}
                  onChange={(v) => setActionId(Number(v))}
                  options={actions.map((action) => ({
                    value: action.id,
                    label: `${action.ref} - ${action.label}`,
                  }))}
                  placeholder="Select an action..."
                  error={!!errors.action}
                />
                {errors.action && (
                  <p className="mt-1 text-sm text-red-600">{errors.action}</p>
                )}
              </div>

              {/* Action Parameters - Dynamic Form */}
              {selectedAction && (
                <>
                  <div>
                    <h4 className="text-sm font-medium text-gray-700 mb-3">
                      Action Parameters
                    </h4>
                    <ParamSchemaForm
                      schema={actionParamSchema}
                      values={actionParameters}
                      onChange={setActionParameters}
                      errors={actionParamErrors}
                      allowTemplates
                    />
                  </div>
                  <div className="rounded-lg border border-gray-200 p-4">
                    <label className="block text-sm font-medium text-gray-700 mb-1">
                      Execution API token permissions
                    </label>
                    <select
                      value={permissionMode}
                      onChange={(e) =>
                        setPermissionMode(e.target.value as PermissionOverrideMode)
                      }
                      className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                    >
                      <option value="inherit">Inherit action default</option>
                      <option value="none">No execution API token</option>
                      <option value="custom">Custom permission set refs</option>
                    </select>
                    {permissionMode === "custom" && (
                      <input
                        value={permissionSetRefsInput}
                        onChange={(e) => setPermissionSetRefsInput(e.target.value)}
                        className="mt-3 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm"
                        placeholder="standard, core.agent_reader"
                      />
                    )}
                    <p className="mt-2 text-xs text-gray-500">
                      Applies to executions created by this rule. Custom refs are
                      comma-separated; use <span className="font-mono">standard</span> for
                      action/pack-scoped keys and artifacts.
                    </p>
                  </div>
                </>
              )}
            </>
          )}
        </div>
      </div>

      {/* Form Actions */}
      <div className="flex justify-end gap-3">
        <button
          type="button"
          onClick={handleCancel}
          className="px-4 py-2 border border-gray-300 rounded-lg text-gray-700 hover:bg-gray-50 transition-colors"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={createRule.isPending || updateRule.isPending}
          className="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {createRule.isPending || updateRule.isPending
            ? "Saving..."
            : isEditing
              ? "Update Rule"
              : "Create Rule"}
        </button>
      </div>
    </form>
  );
}
