import { useState, useEffect, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { usePacks } from "@/hooks/usePacks";
import { useCreateTrigger, useUpdateTrigger } from "@/hooks/useTriggers";
import {
  labelToRef,
  extractLocalRef,
  combinePackLocalRef,
} from "@/lib/format-utils";
import SchemaBuilder from "@/components/common/SchemaBuilder";
import SearchableSelect from "@/components/common/SearchableSelect";
import {
  ActionReferenceVisibility,
  TriggerStringPatch,
  WebhooksService,
} from "@/api";
import type { TriggerResponse, PackSummary } from "@/api";

/** Flat schema format: each key is a parameter name mapped to its definition */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type FlatSchema = Record<string, any>;
type ReferenceVisibility = ActionReferenceVisibility;

interface TriggerFormProps {
  initialData?: TriggerResponse;
  isEditing?: boolean;
}

export default function TriggerForm({
  initialData,
  isEditing = false,
}: TriggerFormProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  // Form fields
  const [packId, setPackId] = useState<number>(0);
  const [localRef, setLocalRef] = useState("");
  const [label, setLabel] = useState("");
  const [description, setDescription] = useState("");
  const [webhookEnabled, setWebhookEnabled] = useState(false);
  const [enabled, setEnabled] = useState(true);
  const [referenceVisibility, setReferenceVisibility] =
    useState<ReferenceVisibility>(ActionReferenceVisibility.PUBLIC);
  const [referenceAllowedPackRefs, setReferenceAllowedPackRefs] = useState("");
  const [paramSchema, setParamSchema] = useState<FlatSchema>({});
  const [outSchema, setOutSchema] = useState<FlatSchema>({});
  const [errors, setErrors] = useState<Record<string, string>>({});

  // Fetch packs
  const { data: packsData } = usePacks({ page: 1, pageSize: 100 });
  const packs = useMemo(() => packsData?.items || [], [packsData?.items]);
  const selectedPack = packs.find((p: PackSummary) => p.id === packId);

  // Mutations
  const createTrigger = useCreateTrigger();
  const updateTrigger = useUpdateTrigger();

  // Initialize form with existing data
  useEffect(() => {
    if (initialData) {
      setLabel(initialData.label || "");
      setDescription(initialData.description || "");
      setWebhookEnabled(initialData.webhook_enabled || false);
      setEnabled(initialData.enabled ?? true);
      setReferenceVisibility(
        (initialData.reference_visibility as ReferenceVisibility | undefined) ??
          ActionReferenceVisibility.PUBLIC,
      );
      setReferenceAllowedPackRefs(
        (initialData.reference_allowed_pack_refs ?? []).join("\n"),
      );
      setParamSchema(initialData.param_schema || {});
      setOutSchema(initialData.out_schema || {});

      if (isEditing) {
        // Find pack by pack_ref
        const pack = packs.find(
          (p: PackSummary) => p.ref === initialData.pack_ref,
        );
        if (pack) {
          setPackId(pack.id);
        }
        // Extract local ref from full ref
        setLocalRef(
          extractLocalRef(initialData.ref, initialData.pack_ref ?? undefined),
        );
      }
    }
  }, [initialData, packs, isEditing]);

  const validateForm = (): boolean => {
    const newErrors: Record<string, string> = {};

    if (!packId) {
      newErrors.pack = "Pack is required";
    }

    if (!label.trim()) {
      newErrors.label = "Label is required";
    }

    if (!localRef.trim()) {
      newErrors.ref = "Reference is required";
    } else if (!/^[a-z0-9_]+$/.test(localRef)) {
      newErrors.ref =
        "Reference must contain only lowercase letters, numbers, and underscores";
    }

    const allowedRefs = parseAllowedPackRefs(referenceAllowedPackRefs);
    if (
      referenceVisibility !== ActionReferenceVisibility.RESTRICTED &&
      allowedRefs.length > 0
    ) {
      newErrors.referenceAllowedPackRefs =
        "Allowed caller packs can only be set when visibility is restricted";
    }

    setErrors(newErrors);
    return Object.keys(newErrors).length === 0;
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!validateForm()) {
      return;
    }

    try {
      const selectedPackData = packs.find((p: PackSummary) => p.id === packId);
      if (!selectedPackData) {
        throw new Error("Selected pack not found");
      }

      const fullRef = combinePackLocalRef(selectedPackData.ref, localRef);
      const allowedRefs = parseAllowedPackRefs(referenceAllowedPackRefs);

      const formData = {
        pack_ref: selectedPackData.ref,
        ref: fullRef,
        label: label.trim(),
        enabled,
        reference_visibility: referenceVisibility,
        reference_allowed_pack_refs: allowedRefs,
        param_schema:
          Object.keys(paramSchema).length > 0 ? paramSchema : undefined,
        out_schema: Object.keys(outSchema).length > 0 ? outSchema : undefined,
      };

      if (isEditing && initialData?.ref) {
        const updateData = {
          ...formData,
          description: description.trim()
            ? { op: TriggerStringPatch.op.SET, value: description.trim() }
            : ({ op: "clear" } as unknown as TriggerStringPatch),
        };

        await updateTrigger.mutateAsync({
          ref: initialData.ref,
          data: updateData,
        });

        // Handle webhook enable/disable separately for updates
        if (webhookEnabled !== initialData?.webhook_enabled) {
          try {
            if (webhookEnabled) {
              await WebhooksService.enableWebhook({ ref: initialData.ref });
            } else {
              await WebhooksService.disableWebhook({ ref: initialData.ref });
            }
            // Invalidate trigger cache to refresh UI with updated webhook status
            queryClient.invalidateQueries({
              queryKey: ["triggers", initialData.ref],
            });
            queryClient.invalidateQueries({ queryKey: ["triggers"] });
          } catch (webhookError) {
            console.error("Failed to update webhook status:", webhookError);
            // Continue anyway - user can update it manually
          }
        }

        // Navigate back to trigger detail page
        navigate(`/triggers/${encodeURIComponent(initialData.ref)}`);
        return;
      } else {
        const createData = {
          ...formData,
          description: description.trim() || undefined,
        };

        const response = await createTrigger.mutateAsync(createData);
        const newTrigger = response?.data;
        if (newTrigger?.ref) {
          // If webhook is enabled, enable it after trigger creation
          if (webhookEnabled) {
            try {
              await WebhooksService.enableWebhook({ ref: newTrigger.ref });
            } catch (webhookError) {
              console.error("Failed to enable webhook:", webhookError);
              // Continue anyway - user can enable it manually
            }
            // Invalidate trigger cache to refresh UI with webhook data
            queryClient.invalidateQueries({
              queryKey: ["triggers", newTrigger.ref],
            });
            queryClient.invalidateQueries({ queryKey: ["triggers"] });
          }
          navigate(`/triggers/${encodeURIComponent(newTrigger.ref)}`);
          return;
        }
      }

      navigate("/triggers");
    } catch (error: unknown) {
      console.error("Error submitting trigger:", error);
      const errMsg =
        error instanceof Error ? error.message : "Failed to save trigger";
      const axiosErr = error as {
        response?: { data?: { message?: string } };
      };
      setErrors({
        submit: axiosErr?.response?.data?.message || errMsg,
      });
    }
  };

  const handleCancel = () => {
    if (isEditing && initialData?.ref) {
      navigate(`/triggers/${encodeURIComponent(initialData.ref)}`);
    } else {
      navigate("/triggers");
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
      <div className="bg-white rounded-lg shadow p-6 space-y-4">
        <h3 className="text-lg font-semibold text-gray-900">
          Basic Information
        </h3>

        {/* Pack Selection */}
        <div>
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
            options={packs.map((pack: PackSummary) => ({
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
        <div>
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
            placeholder="e.g., Webhook Received"
            className={`w-full px-3 py-2 border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 ${
              errors.label ? "border-red-500" : "border-gray-300"
            }`}
          />
          {errors.label && (
            <p className="mt-1 text-sm text-red-600">{errors.label}</p>
          )}
          <p className="mt-1 text-xs text-gray-500">
            Human-readable name for display
          </p>
        </div>

        {/* Reference with Pack Prefix */}
        <div>
          <label
            htmlFor="ref"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Reference <span className="text-red-500">*</span>
          </label>
          <div className="input-with-prefix">
            <span className={`prefix ${errors.ref ? "error" : ""}`}>
              {selectedPack?.ref || "pack"}.
            </span>
            <input
              type="text"
              id="ref"
              value={localRef}
              onChange={(e) => setLocalRef(e.target.value)}
              placeholder="e.g., webhook_received"
              disabled={isEditing}
              className={errors.ref ? "error" : ""}
            />
          </div>
          {errors.ref && (
            <p className="mt-1 text-sm text-red-600">{errors.ref}</p>
          )}
          <p className="mt-1 text-xs text-gray-500">
            Local identifier within the pack. Auto-populated from label.
          </p>
        </div>

        {/* Description */}
        <div>
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
            rows={3}
            placeholder="Describe what this trigger does..."
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
      </div>

      {/* Schema Configuration */}
      <div className="bg-white rounded-lg shadow p-6 space-y-4">
        <h3 className="text-lg font-semibold text-gray-900">
          Schema Configuration
        </h3>

        <div className="bg-blue-50 border border-blue-200 rounded-lg p-4">
          <p className="text-sm text-blue-700">
            Define schemas to validate event parameters and outputs. Leave empty
            for flexible schemas.
          </p>
        </div>

        {/* Parameter Schema */}
        <SchemaBuilder
          label="Parameter Schema"
          value={paramSchema}
          onChange={setParamSchema}
          error={errors.paramSchema}
        />
        <p className="text-xs text-gray-500 -mt-2">
          Define the structure of event parameters that will be passed to this
          trigger
        </p>

        {/* Output Schema */}
        <SchemaBuilder
          label="Output Schema"
          value={outSchema}
          onChange={setOutSchema}
          error={errors.outSchema}
        />
        <p className="text-xs text-gray-500 -mt-2">
          Define the structure of event data that will be produced by this
          trigger
        </p>
      </div>

      {/* Settings */}
      <div className="bg-white rounded-lg shadow p-6 space-y-4">
        <h3 className="text-lg font-semibold text-gray-900">Settings</h3>

        {/* Webhook Enabled */}
        <div className="flex items-center">
          <input
            type="checkbox"
            id="webhookEnabled"
            checked={webhookEnabled}
            onChange={(e) => setWebhookEnabled(e.target.checked)}
            className="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
          />
          <label
            htmlFor="webhookEnabled"
            className="ml-2 block text-sm text-gray-900"
          >
            Enable Webhook
          </label>
        </div>
        <p className="text-xs text-gray-500 ml-6">
          Allow this trigger to be activated via HTTP webhook
        </p>

        {/* Enabled */}
        <div className="flex items-center">
          <input
            type="checkbox"
            id="enabled"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
            className="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
          />
          <label htmlFor="enabled" className="ml-2 block text-sm text-gray-900">
            Enabled
          </label>
        </div>
        <p className="text-xs text-gray-500 ml-6">
          Enable or disable this trigger
        </p>

        {/* Reference Visibility */}
        <div>
          <label
            htmlFor="referenceVisibility"
            className="block text-sm font-medium text-gray-700 mb-1"
          >
            Rule subscription visibility
          </label>
          <select
            id="referenceVisibility"
            value={referenceVisibility}
            onChange={(e) =>
              setReferenceVisibility(e.target.value as ReferenceVisibility)
            }
            className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
          >
            <option value={ActionReferenceVisibility.PUBLIC}>
              Public - rules from any pack
            </option>
            <option value={ActionReferenceVisibility.PRIVATE}>
              Private - only this pack
            </option>
            <option value={ActionReferenceVisibility.RESTRICTED}>
              Restricted - this pack plus allowed packs
            </option>
          </select>
          <p className="mt-1 text-xs text-gray-500">
            Controls which packs can create rules that subscribe to this
            trigger.
          </p>
        </div>

        {referenceVisibility === ActionReferenceVisibility.RESTRICTED && (
          <div>
            <label
              htmlFor="referenceAllowedPackRefs"
              className="block text-sm font-medium text-gray-700 mb-1"
            >
              Allowed caller pack refs
            </label>
            <textarea
              id="referenceAllowedPackRefs"
              value={referenceAllowedPackRefs}
              onChange={(e) => setReferenceAllowedPackRefs(e.target.value)}
              rows={3}
              placeholder="incident_response&#10;deployments"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            {errors.referenceAllowedPackRefs && (
              <p className="mt-1 text-sm text-red-600">
                {errors.referenceAllowedPackRefs}
              </p>
            )}
            <p className="mt-1 text-xs text-gray-500">
              Enter one pack ref per line or separate refs with commas. The
              trigger's own pack is always allowed.
            </p>
          </div>
        )}
      </div>

      {/* Form Actions */}
      <div className="flex justify-end space-x-3">
        <button
          type="button"
          onClick={handleCancel}
          className="px-4 py-2 border border-gray-300 rounded-lg text-gray-700 hover:bg-gray-50"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={createTrigger.isPending || updateTrigger.isPending}
          className="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {createTrigger.isPending || updateTrigger.isPending
            ? "Saving..."
            : isEditing
              ? "Update Trigger"
              : "Create Trigger"}
        </button>
      </div>
    </form>
  );
}

function parseAllowedPackRefs(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((item) => item.trim())
    .filter(Boolean);
}
