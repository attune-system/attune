import { Link } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import TriggerForm from "@/components/forms/TriggerForm";

export default function TriggerCreatePage() {
  return (
    <div className="p-6 max-w-4xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <Link
          to="/triggers"
          className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900 mb-4"
        >
          <ArrowLeft className="h-4 w-4 mr-1" />
          Back to Triggers
        </Link>
        <h1 className="text-3xl font-bold text-gray-900">Create Trigger</h1>
        <p className="mt-2 text-gray-600">
          Create a new ad-hoc trigger for webhooks or workflow-generated events
        </p>
      </div>

      {/* Info Box */}
      <div className="mb-6 bg-blue-50 border border-blue-200 rounded-lg p-4">
        <h3 className="text-sm font-semibold text-blue-900 mb-2">
          About Ad-hoc Triggers
        </h3>
        <ul className="text-sm text-blue-700 space-y-1 list-disc list-inside">
          <li>
            <strong>Webhooks:</strong> Create triggers that can be activated via
            HTTP webhooks
          </li>
          <li>
            <strong>Workflow Events:</strong> Define custom event types for
            workflow orchestration
          </li>
          <li>
            <strong>Schema Validation:</strong> Optionally define JSON schemas
            to validate event data
          </li>
        </ul>
      </div>

      {/* Form */}
      <TriggerForm />
    </div>
  );
}
