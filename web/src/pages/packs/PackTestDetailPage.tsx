import { useParams, Link } from "react-router-dom";
import { ArrowLeft, Loader2 } from "lucide-react";
import { usePackTest } from "@/hooks/usePackTests";
import PackTestResult from "@/components/packs/PackTestResult";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type JsonValue = any;

export default function PackTestDetailPage() {
  const { id } = useParams<{ id: string }>();
  const packId = id ? parseInt(id, 10) : undefined;
  const { data, isLoading, error } = usePackTest(packId);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Loader2 className="w-8 h-8 text-blue-600 animate-spin" />
      </div>
    );
  }

  if (error || !data?.data) {
    return (
      <div className="p-6">
        <Link
          to="/packs"
          className="inline-flex items-center gap-2 text-sm text-blue-600 hover:text-blue-800 mb-4"
        >
          <ArrowLeft className="w-4 h-4" /> Back to packs
        </Link>
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>
            Error:{" "}
            {error ? (error as Error).message : "Pack test execution not found"}
          </p>
        </div>
      </div>
    );
  }

  const execution = data.data;
  const result = (execution.result ?? {}) as JsonValue;

  return (
    <div className="p-6 max-w-7xl mx-auto">
      <div className="mb-6">
        <Link
          to="/packs"
          className="inline-flex items-center gap-2 text-sm text-blue-600 hover:text-blue-800"
        >
          <ArrowLeft className="w-4 h-4" /> Back to packs
        </Link>
        <div className="mt-4 flex items-center gap-3">
          <h1 className="text-2xl font-bold">
            Pack Test Results — v{execution.packVersion}
          </h1>
          <span className="px-2 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-700">
            #{execution.id} · {execution.triggerReason}
          </span>
        </div>
      </div>

      {result && typeof result === "object" && result.status ? (
        <PackTestResult result={result} showDetails />
      ) : (
        <div className="bg-white shadow rounded-lg p-6">
          <dl className="grid grid-cols-2 sm:grid-cols-4 gap-4">
            <div>
              <dt className="text-sm text-gray-500">Total Tests</dt>
              <dd className="text-2xl font-bold">{execution.totalTests}</dd>
            </div>
            <div>
              <dt className="text-sm text-gray-500">Passed</dt>
              <dd className="text-2xl font-bold text-green-600">
                {execution.passed}
              </dd>
            </div>
            {execution.failed > 0 && (
              <div>
                <dt className="text-sm text-gray-500">Failed</dt>
                <dd className="text-2xl font-bold text-red-600">
                  {execution.failed}
                </dd>
              </div>
            )}
            {execution.skipped > 0 && (
              <div>
                <dt className="text-sm text-gray-500">Skipped</dt>
                <dd className="text-2xl font-bold text-gray-600">
                  {execution.skipped}
                </dd>
              </div>
            )}
          </dl>
          <div className="mt-4 text-sm text-gray-600">
            Pass Rate:{" "}
            <span className="font-semibold">
              {(execution.passRate * 100).toFixed(1)}%
            </span>
          </div>
          <p className="mt-4 text-sm text-gray-500">
            Ran at {new Date(execution.executionTime).toLocaleString()}. Run the
            test suites from the pack page for detailed suite/case results.
          </p>
        </div>
      )}
    </div>
  );
}
