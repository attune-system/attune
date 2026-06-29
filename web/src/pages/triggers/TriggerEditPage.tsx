import { useParams, Link } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import TriggerForm from "@/components/forms/TriggerForm";
import { useTrigger } from "@/hooks/useTriggers";

export default function TriggerEditPage() {
  const { ref } = useParams<{ ref: string }>();
  const { data: trigger, isLoading, error } = useTrigger(ref || "");

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error || !trigger?.data) {
    return (
      <div className="p-6 max-w-4xl mx-auto">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {error ? (error as Error).message : "Trigger not found"}</p>
        </div>
      </div>
    );
  }

  if (!trigger.data.is_adhoc) {
    return (
      <div className="p-6 max-w-4xl mx-auto">
        <div className="bg-yellow-50 border border-yellow-200 text-yellow-700 px-4 py-3 rounded">
          <p>
            Only ad-hoc triggers can be edited. This trigger was installed from
            a pack and cannot be modified.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 max-w-4xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <Link
          to={`/triggers/${encodeURIComponent(ref || "")}`}
          className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900 mb-4"
        >
          <ArrowLeft className="h-4 w-4 mr-1" />
          Back to Trigger
        </Link>
        <h1 className="text-3xl font-bold text-gray-900">Edit Trigger</h1>
        <p className="mt-2 text-gray-600">
          Update the configuration for{" "}
          <span className="font-mono text-gray-800">
            {trigger.data.pack_ref}.{trigger.data.label}
          </span>
        </p>
      </div>

      {/* Info Box */}
      <div className="mb-6 bg-blue-50 border border-blue-200 rounded-lg p-4">
        <h3 className="text-sm font-semibold text-blue-900 mb-2">
          About Editing Triggers
        </h3>
        <ul className="text-sm text-blue-700 space-y-1 list-disc list-inside">
          <li>Pack and reference cannot be changed after creation</li>
          <li>Webhook settings can be toggled on or off</li>
          <li>Schema changes will affect future events only</li>
        </ul>
      </div>

      {/* Form */}
      <TriggerForm initialData={trigger.data} isEditing={true} />
    </div>
  );
}
