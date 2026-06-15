import { Link, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import PolicyForm from "@/components/policies/PolicyForm";
import { usePolicy } from "@/hooks/usePolicies";

export default function PolicyEditPage() {
  const { ref } = useParams<{ ref: string }>();
  const policyRef = ref ?? "";
  const { data, isLoading, error } = usePolicy(policyRef);
  const policy = data?.data;

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex h-64 items-center justify-center">
          <div className="h-12 w-12 animate-spin rounded-full border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error || !policy) {
    return (
      <div className="mx-auto max-w-4xl p-6">
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-red-700">
          {error instanceof Error ? error.message : "Policy not found"}
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-6xl p-6">
      <div className="mb-6">
        <Link
          to={`/policies/${encodeURIComponent(policy.ref)}`}
          className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900"
        >
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back to Policy
        </Link>
        <h1 className="mt-4 text-3xl font-bold text-gray-900">Edit Policy</h1>
        <p className="mt-2 text-gray-600">
          Update policy behavior for{" "}
          <span className="font-mono text-gray-800">{policy.ref}</span>.
        </p>
      </div>

      <PolicyForm initialData={policy} isEditing />
    </div>
  );
}
