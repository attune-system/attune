import { Link } from "react-router-dom";
import { ArrowLeft, Info } from "lucide-react";
import PolicyForm from "@/components/policies/PolicyForm";

export default function PolicyCreatePage() {
  return (
    <div className="mx-auto max-w-6xl p-6">
      <div className="mb-6">
        <Link
          to="/policies"
          className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900"
        >
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back to Policies
        </Link>
        <h1 className="mt-4 text-3xl font-bold text-gray-900">Create Policy</h1>
        <p className="mt-2 text-gray-600">
          Create a scoped execution policy using guided controls for every
          policy feature.
        </p>
      </div>

      <div className="mb-6 rounded-lg border border-blue-200 bg-blue-50 p-5">
        <div className="flex items-start gap-3">
          <Info className="mt-0.5 h-5 w-5 flex-shrink-0 text-blue-600" />
          <div className="text-sm text-blue-900">
            <h2 className="font-semibold">Policy precedence</h2>
            <p className="mt-1 text-blue-800">
              Attune applies one effective policy. Action policies override pack
              policies, pack policies override global policies, and higher
              priority wins within the same scope.
            </p>
          </div>
        </div>
      </div>

      <PolicyForm />
    </div>
  );
}
