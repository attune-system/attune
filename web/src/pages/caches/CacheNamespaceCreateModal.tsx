import { useState } from "react";
import { X } from "lucide-react";
import { OwnerType, type CacheNamespaceResponse } from "@/api";
import { useCreateCacheNamespace } from "@/hooks/useCaches";
import OwnerScopeSelector, {
  type OwnerScopeValue,
} from "@/components/caches/OwnerScopeSelector";
import {
  getCacheErrorMessage,
  isValidMaxRetainedGenerations,
  isValidNamespaceName,
  MIN_RETAINED_GENERATIONS,
} from "@/components/caches/cacheUtils";

interface CacheNamespaceCreateModalProps {
  onClose: () => void;
  onCreated: (namespace: CacheNamespaceResponse) => void;
}

// Mirrors `CacheNamespacePolicy::default()` in
// crates/common/src/repositories/cache.rs so the form's placeholders/defaults
// stay consistent with what the server would otherwise apply.
const DEFAULT_POLICY = {
  freshness_target_seconds: 3600,
  max_records_per_generation: 200_000,
  max_generation_bytes: 512 * 1024 * 1024,
  max_retained_bytes: 2 * 1024 * 1024 * 1024,
  max_retained_generations: 5,
  max_staging_generations: 2,
};

export default function CacheNamespaceCreateModal({
  onClose,
  onCreated,
}: CacheNamespaceCreateModalProps) {
  const [owner, setOwner] = useState<OwnerScopeValue>({
    ownerType: OwnerType.PACK,
    ownerRef: "",
  });
  const [namespace, setNamespace] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [policy, setPolicy] = useState(DEFAULT_POLICY);
  const [error, setError] = useState<string | null>(null);

  const createNamespace = useCreateCacheNamespace();

  const requiresOwnerRef =
    owner.ownerType === OwnerType.PACK ||
    owner.ownerType === OwnerType.ACTION ||
    owner.ownerType === OwnerType.SENSOR;

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);

    if (!isValidNamespaceName(namespace)) {
      setError(
        "Namespace must be lowercase letters, numbers, '.', '_' or '-', starting with a letter or number (max 128 characters).",
      );
      return;
    }
    if (requiresOwnerRef && !owner.ownerRef) {
      setError("Select an owner reference for this owner scope.");
      return;
    }
    if (!isValidMaxRetainedGenerations(policy.max_retained_generations)) {
      setError(
        `Max retained generations must be at least ${MIN_RETAINED_GENERATIONS}.`,
      );
      return;
    }

    try {
      const response = await createNamespace.mutateAsync({
        owner_type: owner.ownerType,
        owner_ref: requiresOwnerRef ? owner.ownerRef : undefined,
        namespace,
        ...policy,
      });
      onCreated(response.data);
    } catch (err) {
      setError(getCacheErrorMessage(err, "Failed to create cache namespace"));
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div className="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-lg bg-white shadow-xl">
        <div className="flex items-center justify-between border-b border-gray-200 p-6">
          <h2 className="text-2xl font-bold text-gray-900">
            Create Cache Namespace
          </h2>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600"
            aria-label="Close"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="space-y-5 p-6">
          {error && (
            <div className="rounded-md bg-red-50 px-3 py-2 text-sm text-red-700">
              {error}
            </div>
          )}

          <OwnerScopeSelector value={owner} onChange={setOwner} />

          <div>
            <label className="block text-sm font-medium text-gray-700">
              Namespace
            </label>
            <input
              type="text"
              value={namespace}
              onChange={(event) => setNamespace(event.target.value)}
              placeholder="e.g. users"
              className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-teal-500"
            />
            <p className="mt-1 text-xs text-gray-500">
              Immutable once created. Owner scope + namespace together are the
              cache's authorization and lifecycle boundary.
            </p>
          </div>

          <div>
            <button
              type="button"
              onClick={() => setShowAdvanced((prev) => !prev)}
              className="text-sm font-medium text-teal-700 hover:text-teal-900"
            >
              {showAdvanced ? "Hide" : "Show"} advanced policy (freshness &amp;
              quotas)
            </button>
          </div>

          {showAdvanced && (
            <div className="grid grid-cols-1 gap-4 rounded-md border border-gray-200 bg-gray-50 p-4 sm:grid-cols-2">
              <NumberField
                label="Freshness target (seconds)"
                value={policy.freshness_target_seconds}
                onChange={(value) =>
                  setPolicy((prev) => ({
                    ...prev,
                    freshness_target_seconds: value,
                  }))
                }
              />
              <NumberField
                label="Max records per generation"
                value={policy.max_records_per_generation}
                onChange={(value) =>
                  setPolicy((prev) => ({
                    ...prev,
                    max_records_per_generation: value,
                  }))
                }
              />
              <NumberField
                label="Max generation bytes"
                value={policy.max_generation_bytes}
                onChange={(value) =>
                  setPolicy((prev) => ({
                    ...prev,
                    max_generation_bytes: value,
                  }))
                }
              />
              <NumberField
                label="Max retained bytes"
                value={policy.max_retained_bytes}
                onChange={(value) =>
                  setPolicy((prev) => ({ ...prev, max_retained_bytes: value }))
                }
              />
              <NumberField
                label="Max retained generations"
                min={MIN_RETAINED_GENERATIONS}
                value={policy.max_retained_generations}
                onChange={(value) =>
                  setPolicy((prev) => ({
                    ...prev,
                    max_retained_generations: value,
                  }))
                }
              />
              <NumberField
                label="Max concurrent staging generations"
                value={policy.max_staging_generations}
                onChange={(value) =>
                  setPolicy((prev) => ({
                    ...prev,
                    max_staging_generations: value,
                  }))
                }
              />
            </div>
          )}

          <div className="flex justify-end gap-3 border-t border-gray-200 pt-4">
            <button
              type="button"
              onClick={onClose}
              className="rounded-lg px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-100"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={createNamespace.isPending}
              className="rounded-lg bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
            >
              {createNamespace.isPending ? "Creating…" : "Create Namespace"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

function NumberField({
  label,
  min = 0,
  value,
  onChange,
}: {
  label: string;
  min?: number;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <div>
      <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
        {label}
      </label>
      <input
        type="number"
        min={min}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
        className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
      />
    </div>
  );
}
