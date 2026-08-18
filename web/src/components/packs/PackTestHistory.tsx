import { useState } from "react";
import { Link } from "react-router-dom";
import { Calendar, Clock } from "lucide-react";
import PackTestBadge from "./PackTestBadge";

interface TestExecution {
  id: number;
  packId: number;
  packVersion: string;
  executionTime: string;
  triggerReason: string;
  totalTests: number;
  passed: number;
  failed: number;
  skipped: number;
  passRate: number;
  durationMs: number;
  status?: string;
}

interface PackTestHistoryProps {
  executions: TestExecution[];
  isLoading?: boolean;
  onLoadMore?: () => void;
  hasMore?: boolean;
}

export default function PackTestHistory({
  executions,
  isLoading = false,
  onLoadMore,
  hasMore = false,
}: PackTestHistoryProps) {
  const [expandedId, setExpandedId] = useState<number | null>(null);

  const formatDuration = (ms: number) => {
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(2)}s`;
  };

  const getTriggerBadgeColor = (trigger: string) => {
    switch (trigger.toLowerCase()) {
      case "register":
        return "bg-blue-100 text-blue-800";
      case "manual":
        return "bg-purple-100 text-purple-800";
      case "ci":
        return "bg-green-100 text-green-800";
      case "schedule":
        return "bg-yellow-100 text-yellow-800";
      default:
        return "bg-gray-100 text-gray-800";
    }
  };

  const getStatus = (execution: TestExecution): string => {
    if (execution.status) return execution.status;
    if (execution.failed > 0) return "failed";
    if (execution.passed === execution.totalTests) return "passed";
    return "partial";
  };

  if (isLoading && executions.length === 0) {
    return (
      <div className="bg-white shadow rounded-lg p-8">
        <div className="flex items-center justify-center">
          <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (executions.length === 0) {
    return (
      <div className="bg-white shadow rounded-lg p-8">
        <div className="text-center text-gray-500">
          <p className="text-lg font-medium mb-2">No test history</p>
          <p className="text-sm">
            Test executions will appear here once tests are run.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="bg-white shadow rounded-lg overflow-hidden">
      <div className="divide-y divide-gray-200">
        {executions.map((execution) => {
          const status = getStatus(execution);
          const isExpanded = expandedId === execution.id;

          return (
            <div
              key={execution.id}
              className="hover:bg-gray-50 transition-colors"
            >
              <button
                onClick={() => setExpandedId(isExpanded ? null : execution.id)}
                className="w-full px-6 py-4 text-left"
              >
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-4 flex-1">
                    {/* Status Badge */}
                    <PackTestBadge
                      status={status}
                      passed={execution.passed}
                      total={execution.totalTests}
                      size="sm"
                    />

                    {/* Test Info */}
                    <div className="flex-1">
                      <div className="flex items-center gap-2 mb-1">
                        <span className="text-sm font-medium text-gray-900">
                          Version {execution.packVersion}
                        </span>
                        <span
                          className={`px-2 py-0.5 text-xs rounded-full ${getTriggerBadgeColor(
                            execution.triggerReason,
                          )}`}
                        >
                          {execution.triggerReason}
                        </span>
                      </div>
                      <div className="flex items-center gap-4 text-xs text-gray-500">
                        <div className="flex items-center gap-1">
                          <Calendar className="w-3 h-3" />
                          <span>
                            {new Date(
                              execution.executionTime,
                            ).toLocaleDateString()}
                            {" at "}
                            {new Date(
                              execution.executionTime,
                            ).toLocaleTimeString()}
                          </span>
                        </div>
                        <div className="flex items-center gap-1">
                          <Clock className="w-3 h-3" />
                          <span>{formatDuration(execution.durationMs)}</span>
                        </div>
                      </div>
                    </div>

                    {/* Pass Rate */}
                    <div className="text-right">
                      <div className="text-sm font-semibold text-gray-900">
                        {(execution.passRate * 100).toFixed(1)}%
                      </div>
                      <div className="text-xs text-gray-500">pass rate</div>
                    </div>
                  </div>
                </div>

                {/* Expanded Details */}
                {isExpanded && (
                  <div className="mt-4 pt-4 border-t border-gray-200">
                    <div className="grid grid-cols-4 gap-4 text-center">
                      <div>
                        <div className="text-2xl font-bold text-gray-900">
                          {execution.totalTests}
                        </div>
                        <div className="text-xs text-gray-500 mt-1">Total</div>
                      </div>
                      <div>
                        <div className="text-2xl font-bold text-green-600">
                          {execution.passed}
                        </div>
                        <div className="text-xs text-gray-500 mt-1">Passed</div>
                      </div>
                      {execution.failed > 0 && (
                        <div>
                          <div className="text-2xl font-bold text-red-600">
                            {execution.failed}
                          </div>
                          <div className="text-xs text-gray-500 mt-1">
                            Failed
                          </div>
                        </div>
                      )}
                      {execution.skipped > 0 && (
                        <div>
                          <div className="text-2xl font-bold text-gray-600">
                            {execution.skipped}
                          </div>
                          <div className="text-xs text-gray-500 mt-1">
                            Skipped
                          </div>
                        </div>
                      )}
                    </div>

                    <div className="mt-4 flex justify-end">
                      <Link
                        to={`/packs/tests/${execution.id}`}
                        className="text-sm text-blue-600 hover:text-blue-800 font-medium"
                        onClick={(e) => e.stopPropagation()}
                      >
                        View Full Results →
                      </Link>
                    </div>
                  </div>
                )}
              </button>
            </div>
          );
        })}
      </div>

      {/* Load More Button */}
      {hasMore && (
        <div className="p-4 border-t border-gray-200 bg-gray-50">
          <button
            onClick={onLoadMore}
            disabled={isLoading}
            className="w-full px-4 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {isLoading ? "Loading..." : "Load More"}
          </button>
        </div>
      )}
    </div>
  );
}
