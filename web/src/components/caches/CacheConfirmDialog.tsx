import { useState } from "react";
import { AlertTriangle, X } from "lucide-react";

export interface CacheConfirmImpactRow {
  label: string;
  value: string;
}

interface CacheConfirmDialogProps {
  title: string;
  description?: string;
  /** Count/byte (or similar) impact rows shown before the user confirms. */
  impact?: CacheConfirmImpactRow[];
  requireReason?: boolean;
  reasonLabel?: string;
  reasonPlaceholder?: string;
  /** When set, the confirm button stays disabled until this exact phrase is typed. */
  confirmPhrase?: string;
  confirmLabel?: string;
  tone?: "danger" | "warning";
  isSubmitting?: boolean;
  errorMessage?: string | null;
  onCancel: () => void;
  onConfirm: (reason: string) => void;
}

const TONE_STYLES: Record<
  NonNullable<CacheConfirmDialogProps["tone"]>,
  { icon: string; button: string }
> = {
  danger: {
    icon: "text-red-600",
    button: "bg-red-600 hover:bg-red-700 disabled:bg-red-300",
  },
  warning: {
    icon: "text-amber-600",
    button: "bg-amber-600 hover:bg-amber-700 disabled:bg-amber-300",
  },
};

/**
 * Shared confirmation dialog for cache destructive/publication actions
 * (abandon a refresh, delete a namespace, promote with a known conflict).
 * Never renders record/entry values — only counts, bytes, and metadata.
 */
export default function CacheConfirmDialog({
  title,
  description,
  impact,
  requireReason = false,
  reasonLabel = "Reason",
  reasonPlaceholder = "Why is this action being taken?",
  confirmPhrase,
  confirmLabel = "Confirm",
  tone = "warning",
  isSubmitting = false,
  errorMessage,
  onCancel,
  onConfirm,
}: CacheConfirmDialogProps) {
  const [reason, setReason] = useState("");
  const [typedPhrase, setTypedPhrase] = useState("");

  const toneStyles = TONE_STYLES[tone];
  const reasonSatisfied = !requireReason || reason.trim().length > 0;
  const phraseSatisfied = !confirmPhrase || typedPhrase === confirmPhrase;
  const canConfirm = reasonSatisfied && phraseSatisfied && !isSubmitting;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div className="w-full max-w-lg rounded-lg bg-white shadow-xl">
        <div className="flex items-start justify-between gap-4 border-b border-gray-200 p-5">
          <div className="flex items-start gap-3">
            <AlertTriangle
              className={`mt-0.5 h-5 w-5 shrink-0 ${toneStyles.icon}`}
            />
            <div>
              <h2 className="text-lg font-semibold text-gray-900">{title}</h2>
              {description && (
                <p className="mt-1 text-sm text-gray-600">{description}</p>
              )}
            </div>
          </div>
          <button
            type="button"
            onClick={onCancel}
            className="text-gray-400 hover:text-gray-600"
            aria-label="Close"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <div className="space-y-4 p-5">
          {impact && impact.length > 0 && (
            <dl className="grid grid-cols-2 gap-3 rounded-md bg-gray-50 p-3 text-sm">
              {impact.map((row) => (
                <div key={row.label}>
                  <dt className="text-xs uppercase tracking-wide text-gray-500">
                    {row.label}
                  </dt>
                  <dd className="font-medium text-gray-900">{row.value}</dd>
                </div>
              ))}
            </dl>
          )}

          {requireReason && (
            <div>
              <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                {reasonLabel}
              </label>
              <textarea
                value={reason}
                onChange={(event) => setReason(event.target.value)}
                placeholder={reasonPlaceholder}
                rows={2}
                className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
          )}

          {confirmPhrase && (
            <div>
              <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                Type <span className="font-mono">{confirmPhrase}</span> to
                confirm
              </label>
              <input
                value={typedPhrase}
                onChange={(event) => setTypedPhrase(event.target.value)}
                className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 font-mono text-sm"
              />
            </div>
          )}

          {errorMessage && (
            <p className="rounded-md bg-red-50 px-3 py-2 text-sm text-red-700">
              {errorMessage}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-gray-200 p-4">
          <button
            type="button"
            onClick={onCancel}
            disabled={isSubmitting}
            className="rounded-md px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-100"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => onConfirm(reason.trim())}
            disabled={!canConfirm}
            className={`rounded-md px-4 py-2 text-sm font-medium text-white disabled:cursor-not-allowed ${toneStyles.button}`}
          >
            {isSubmitting ? "Working…" : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
