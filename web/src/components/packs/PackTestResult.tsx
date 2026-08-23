import { useState } from "react";
import {
  CheckCircle,
  XCircle,
  Clock,
  ChevronDown,
  ChevronRight,
} from "lucide-react";

interface TestCaseResult {
  name: string;
  status: "passed" | "failed" | "skipped" | "error";
  duration_ms: number;
  error_message?: string;
  stdout?: string;
  stderr?: string;
}

interface TestSuiteResult {
  name: string;
  runner_type?: string;
  runnerType?: string;
  total: number;
  passed: number;
  failed: number;
  skipped: number;
  duration_ms?: number;
  durationMs?: number;
  test_cases?: TestCaseResult[];
  testCases?: TestCaseResult[];
}

interface PackTestResultData {
  pack_ref: string;
  packRef?: string;
  pack_version: string;
  packVersion?: string;
  execution_time: string;
  executionTime?: string;
  status: string;
  total_tests?: number;
  totalTests?: number;
  passed: number;
  failed: number;
  skipped: number;
  pass_rate?: number;
  passRate?: number;
  duration_ms?: number;
  durationMs?: number;
  test_suites?: TestSuiteResult[];
  testSuites?: TestSuiteResult[];
}

interface PackTestResultProps {
  result: PackTestResultData;
  showDetails?: boolean;
}

export default function PackTestResult({
  result,
  showDetails = false,
}: PackTestResultProps) {
  const [expandedSuites, setExpandedSuites] = useState<Set<string>>(new Set());
  // Older/no-suite test results omit this field entirely. Treat that shape as
  // an empty suite list so the summary page remains renderable.
  const testSuites = (result.test_suites ?? result.testSuites ?? []).map(
    (suite) => ({
      ...suite,
      runner_type: suite.runner_type ?? suite.runnerType ?? "unknown",
      duration_ms: suite.duration_ms ?? suite.durationMs ?? 0,
      test_cases: suite.test_cases ?? suite.testCases ?? [],
    }),
  );
  const executionTime = result.execution_time ?? result.executionTime;
  const totalTests = result.total_tests ?? result.totalTests ?? 0;
  const passRate = result.pass_rate ?? result.passRate ?? 0;
  const durationMs = result.duration_ms ?? result.durationMs ?? 0;

  const toggleSuite = (suiteName: string) => {
    setExpandedSuites((prev) => {
      const next = new Set(prev);
      if (next.has(suiteName)) {
        next.delete(suiteName);
      } else {
        next.add(suiteName);
      }
      return next;
    });
  };

  const getStatusColor = (status: string) => {
    switch (status) {
      case "passed":
        return "text-green-600 bg-green-50";
      case "failed":
        return "text-red-600 bg-red-50";
      case "skipped":
        return "text-gray-600 bg-gray-50";
      default:
        return "text-yellow-600 bg-yellow-50";
    }
  };

  const getStatusIcon = (status: string) => {
    switch (status) {
      case "passed":
        return <CheckCircle className="w-5 h-5 text-green-600" />;
      case "failed":
        return <XCircle className="w-5 h-5 text-red-600" />;
      default:
        return <Clock className="w-5 h-5 text-gray-600" />;
    }
  };

  const formatDuration = (ms: number) => {
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(2)}s`;
  };

  return (
    <div className="bg-white shadow rounded-lg overflow-hidden">
      {/* Summary Header */}
      <div className="p-6 border-b border-gray-200">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-3">
            {getStatusIcon(result.status)}
            <div>
              <h3 className="text-lg font-semibold">
                {result.status === "passed"
                  ? "All Tests Passed"
                  : "Tests Failed"}
              </h3>
              <p className="text-sm text-gray-500">
                {executionTime
                  ? new Date(executionTime).toLocaleString()
                  : "Unknown time"}
              </p>
            </div>
          </div>
          <span
            className={`px-3 py-1 rounded-full text-sm font-medium ${getStatusColor(
              result.status,
            )}`}
          >
            {result.status.toUpperCase()}
          </span>
        </div>

        {/* Test Statistics */}
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
          <div>
            <div className="text-sm text-gray-500">Total Tests</div>
            <div className="text-2xl font-bold">{totalTests}</div>
          </div>
          <div>
            <div className="text-sm text-gray-500">Passed</div>
            <div className="text-2xl font-bold text-green-600">
              {result.passed}
            </div>
          </div>
          {result.failed > 0 && (
            <div>
              <div className="text-sm text-gray-500">Failed</div>
              <div className="text-2xl font-bold text-red-600">
                {result.failed}
              </div>
            </div>
          )}
          {result.skipped > 0 && (
            <div>
              <div className="text-sm text-gray-500">Skipped</div>
              <div className="text-2xl font-bold text-gray-600">
                {result.skipped}
              </div>
            </div>
          )}
        </div>

        <div className="mt-4 flex items-center gap-4 text-sm text-gray-600">
          <div>
            Pass Rate:{" "}
            <span className="font-semibold">
              {(passRate * 100).toFixed(1)}%
            </span>
          </div>
          <div>
            Duration:{" "}
            <span className="font-semibold">{formatDuration(durationMs)}</span>
          </div>
        </div>
      </div>

      {/* Detailed Results */}
      {showDetails && testSuites.length > 0 && (
        <div className="p-6">
          <h4 className="text-sm font-semibold text-gray-700 mb-4">
            Test Suites ({testSuites.length})
          </h4>
          <div className="space-y-3">
            {testSuites.map((suite) => (
              <div
                key={suite.name}
                className="border border-gray-200 rounded-lg overflow-hidden"
              >
                {/* Suite Header */}
                <button
                  onClick={() => toggleSuite(suite.name)}
                  className="w-full px-4 py-3 bg-gray-50 hover:bg-gray-100 flex items-center justify-between transition-colors"
                >
                  <div className="flex items-center gap-3">
                    {expandedSuites.has(suite.name) ? (
                      <ChevronDown className="w-4 h-4 text-gray-500" />
                    ) : (
                      <ChevronRight className="w-4 h-4 text-gray-500" />
                    )}
                    <div className="text-left">
                      <div className="font-medium text-gray-900">
                        {suite.name}
                      </div>
                      <div className="text-xs text-gray-500">
                        {suite.runner_type} •{" "}
                        {formatDuration(suite.duration_ms)}
                      </div>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 text-sm">
                    <span className="text-green-600">
                      {suite.passed} passed
                    </span>
                    {suite.failed > 0 && (
                      <span className="text-red-600">
                        {suite.failed} failed
                      </span>
                    )}
                    {suite.skipped > 0 && (
                      <span className="text-gray-600">
                        {suite.skipped} skipped
                      </span>
                    )}
                  </div>
                </button>

                {/* Test Cases */}
                {expandedSuites.has(suite.name) && (
                  <div className="divide-y divide-gray-200">
                    {suite.test_cases.map((testCase, idx) => (
                      <div key={idx} className="px-4 py-3 bg-white">
                        <div className="flex items-start justify-between">
                          <div className="flex items-start gap-2 flex-1">
                            {testCase.status === "passed" ? (
                              <CheckCircle className="w-4 h-4 text-green-600 mt-0.5 flex-shrink-0" />
                            ) : testCase.status === "failed" ? (
                              <XCircle className="w-4 h-4 text-red-600 mt-0.5 flex-shrink-0" />
                            ) : (
                              <Clock className="w-4 h-4 text-gray-400 mt-0.5 flex-shrink-0" />
                            )}
                            <div className="flex-1 min-w-0">
                              <div className="font-mono text-sm text-gray-900">
                                {testCase.name}
                              </div>
                              {testCase.error_message && (
                                <div className="mt-2 p-2 bg-red-50 border border-red-200 rounded text-xs">
                                  <div className="font-semibold text-red-800 mb-1">
                                    Error:
                                  </div>
                                  <pre className="text-red-700 whitespace-pre-wrap break-words font-mono">
                                    {testCase.error_message}
                                  </pre>
                                </div>
                              )}
                              {testCase.stderr && (
                                <div className="mt-2 p-2 bg-gray-50 border border-gray-200 rounded text-xs">
                                  <div className="font-semibold text-gray-800 mb-1">
                                    stderr:
                                  </div>
                                  <pre className="text-gray-700 whitespace-pre-wrap break-words font-mono">
                                    {testCase.stderr}
                                  </pre>
                                </div>
                              )}
                            </div>
                          </div>
                          <span className="text-xs text-gray-500 ml-4 flex-shrink-0">
                            {formatDuration(testCase.duration_ms)}
                          </span>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
