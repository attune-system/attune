import { Link, useParams } from "react-router-dom";
import {
  useSensors,
  useSensor,
  useDeleteSensor,
  useUpdateSensor,
} from "@/hooks/useSensors";
import { useSensorLog, useSensorLogs } from "@/hooks/useSensorLogs";
import { useState, useMemo } from "react";
import type { SensorResponse, SensorSummary, UpdateSensorRequest } from "@/api";
import {
  LogRetentionLimitPatch,
  LogRetentionPolicyPatch,
  RetentionPolicyType,
} from "@/api";
import { ChevronDown, ChevronRight, Search, Settings, X } from "lucide-react";
import OnOffSwitch from "@/components/common/OnOffSwitch";
import RetentionPolicyControls from "@/components/common/RetentionPolicyControls";
import {
  formatRetention,
  type RetentionPolicy,
} from "@/components/common/retentionPolicy";
import PackIcon from "@/components/common/PackIcon";

export default function SensorsPage() {
  const { ref } = useParams<{ ref?: string }>();
  const { data, isLoading, error } = useSensors({});
  const sensors = useMemo(() => data?.items || [], [data?.items]);
  const [collapsedPacks, setCollapsedPacks] = useState<Set<string>>(new Set());
  const [searchQuery, setSearchQuery] = useState("");

  // Filter sensors based on search query
  const filteredSensors = useMemo(() => {
    if (!searchQuery.trim()) return sensors;
    const query = searchQuery.toLowerCase();
    return sensors.filter((sensor: SensorSummary) => {
      return (
        sensor.label?.toLowerCase().includes(query) ||
        sensor.ref?.toLowerCase().includes(query) ||
        sensor.description?.toLowerCase().includes(query) ||
        sensor.pack_ref?.toLowerCase().includes(query)
      );
    });
  }, [sensors, searchQuery]);

  // Group filtered sensors by pack
  const sensorsByPack = useMemo(() => {
    const grouped = new Map<string, SensorSummary[]>();
    filteredSensors.forEach((sensor: SensorSummary) => {
      const packRef = sensor.pack_ref || "unknown";
      if (!grouped.has(packRef)) {
        grouped.set(packRef, []);
      }
      grouped.get(packRef)!.push(sensor);
    });
    // Sort packs alphabetically
    return new Map(
      [...grouped.entries()].sort((a, b) => a[0].localeCompare(b[0])),
    );
  }, [filteredSensors]);

  const togglePack = (packRef: string) => {
    setCollapsedPacks((prev) => {
      const next = new Set(prev);
      if (next.has(packRef)) {
        next.delete(packRef);
      } else {
        next.add(packRef);
      }
      return next;
    });
  };

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {(error as Error).message}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full">
      {/* Left sidebar - Sensors List */}
      <div className="w-96 border-r border-gray-200 overflow-y-auto bg-gray-50">
        <div className="p-4 border-b border-gray-200 bg-white sticky top-0 z-10">
          <h1 className="text-2xl font-bold">Sensors</h1>
          <p className="text-sm text-gray-600 mt-1">
            {filteredSensors.length} of {sensors.length} sensors
          </p>

          {/* Search Bar */}
          <div className="mt-3 relative">
            <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
              <Search className="h-4 w-4 text-gray-400" />
            </div>
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search sensors..."
              className="block w-full pl-10 pr-10 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery("")}
                className="absolute inset-y-0 right-0 pr-3 flex items-center"
              >
                <X className="h-4 w-4 text-gray-400 hover:text-gray-600" />
              </button>
            )}
          </div>
        </div>
        <div className="p-2">
          {sensors.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No sensors found</p>
            </div>
          ) : filteredSensors.length === 0 ? (
            <div className="bg-white p-8 text-center rounded-lg shadow-sm m-2">
              <p className="text-gray-500">No sensors match your search</p>
              <button
                onClick={() => setSearchQuery("")}
                className="mt-2 text-sm text-blue-600 hover:text-blue-800"
              >
                Clear search
              </button>
            </div>
          ) : (
            <div className="space-y-2">
              {Array.from(sensorsByPack.entries()).map(
                ([packRef, packSensors]) => {
                  const isCollapsed = collapsedPacks.has(packRef);
                  return (
                    <div
                      key={packRef}
                      className="bg-white rounded-lg shadow-sm overflow-hidden"
                    >
                      {/* Pack Header */}
                      <button
                        onClick={() => togglePack(packRef)}
                        className="w-full px-3 py-2 flex items-center justify-between hover:bg-gray-50 transition-colors border-b border-gray-200"
                      >
                        <div className="flex items-center gap-2">
                          {isCollapsed ? (
                            <ChevronRight className="w-4 h-4 text-gray-500" />
                          ) : (
                            <ChevronDown className="w-4 h-4 text-gray-500" />
                          )}
                          <PackIcon packRef={packRef} size="xs" />
                          <span className="font-semibold text-sm text-gray-900">
                            {packRef}
                          </span>
                        </div>
                        <span className="text-xs text-gray-500 bg-gray-100 px-2 py-0.5 rounded">
                          {packSensors.length}
                        </span>
                      </button>

                      {/* Sensors List */}
                      {!isCollapsed && (
                        <div className="p-1">
                          {packSensors.map((sensor: SensorSummary) => (
                            <Link
                              key={sensor.id}
                              to={`/sensors/${sensor.ref}`}
                              className={`block p-3 rounded transition-colors ${
                                ref === sensor.ref
                                  ? "bg-blue-50 border-2 border-blue-500"
                                  : "border-2 border-transparent hover:bg-gray-50"
                              }`}
                            >
                              <div className="flex items-center justify-between">
                                <div className="min-w-0 flex items-center gap-2">
                                  <PackIcon packRef={sensor.pack_ref} size="sm" />
                                  <div className="font-medium text-sm text-gray-900 truncate">
                                    {sensor.label}
                                  </div>
                                </div>
                                <span
                                  className={`ml-2 inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium ${
                                    sensor.enabled
                                      ? "bg-green-100 text-green-800"
                                      : "bg-gray-100 text-gray-800"
                                  }`}
                                >
                                  {sensor.enabled ? "Enabled" : "Disabled"}
                                </span>
                              </div>
                              <div className="font-mono text-xs text-gray-500 mt-1 truncate">
                                {sensor.ref}
                              </div>
                              {sensor.description && (
                                <div className="text-xs text-gray-400 mt-1 line-clamp-2">
                                  {sensor.description}
                                </div>
                              )}
                            </Link>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                },
              )}
            </div>
          )}
        </div>
      </div>

      {/* Right panel - Sensor Detail or Empty State */}
      <div className="flex-1 overflow-y-auto">
        {ref ? (
          <SensorDetail sensorRef={ref} />
        ) : (
          <div className="flex items-center justify-center h-full">
            <div className="text-center text-gray-500">
              <svg
                className="mx-auto h-12 w-12 text-gray-400"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth={2}
                  d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
                />
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth={2}
                  d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z"
                />
              </svg>
              <h3 className="mt-2 text-sm font-medium text-gray-900">
                No sensor selected
              </h3>
              <p className="mt-1 text-sm text-gray-500">
                Select a sensor from the list to view its details
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function SensorDetail({ sensorRef }: { sensorRef: string }) {
  const { data: sensor, isLoading, error } = useSensor(sensorRef);
  const deleteSensor = useDeleteSensor();
  const updateSensor = useUpdateSensor();
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [showConfigureModal, setShowConfigureModal] = useState(false);
  const [logStream, setLogStream] = useState<"stdout" | "stderr">("stderr");
  const [followLogs, setFollowLogs] = useState(false);
  const { data: logSummary } = useSensorLogs(sensorRef, Boolean(sensorRef));
  const selectedLogEntry = logSummary?.logs.find(
    (log) => log.stream === logStream,
  );
  const selectedLogAvailable = Boolean(selectedLogEntry?.artifact_id);
  const { data: logContent, isLoading: logLoading } = useSensorLog(
    sensorRef,
    logStream,
    200,
    followLogs,
    selectedLogAvailable,
  );

  const handleDelete = async () => {
    try {
      await deleteSensor.mutateAsync(sensorRef);
      window.location.href = "/sensors";
    } catch (err) {
      console.error("Failed to delete sensor:", err);
    }
  };

  const handleToggleEnabled = async (enabled: boolean) => {
    try {
      await updateSensor.mutateAsync({
        ref: sensorRef,
        data: { enabled },
      });
    } catch (err) {
      console.error("Failed to toggle sensor enabled status:", err);
    }
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  if (error || !sensor) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>Error: {error ? (error as Error).message : "Sensor not found"}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 max-w-7xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <h1 className="text-3xl font-bold">
              <span className="text-gray-500">{sensor.data?.pack_ref}.</span>
              {sensor.data?.label}
            </h1>
            <div className="inline-flex items-center">
              <OnOffSwitch
                checked={sensor.data?.enabled || false}
                disabled={updateSensor.isPending}
                ariaLabel="Sensor enabled"
                onChange={(checked) => {
                  void handleToggleEnabled(checked);
                }}
              />
              <span className="ms-3 text-sm font-medium">
                {updateSensor.isPending ? (
                  <span className="text-gray-400">Updating...</span>
                ) : (
                  <span
                    className={
                      sensor.data?.enabled ? "text-green-700" : "text-gray-700"
                    }
                  >
                    {sensor.data?.enabled ? "Enabled" : "Disabled"}
                  </span>
                )}
              </span>
            </div>
          </div>
          <div className="flex gap-2">
            <button
              onClick={() => setShowConfigureModal(true)}
              className="inline-flex items-center gap-2 px-4 py-2 bg-gray-100 text-gray-700 rounded hover:bg-gray-200"
            >
              <Settings className="h-4 w-4" />
              Configure
            </button>
            <button
              onClick={() => setShowDeleteConfirm(true)}
              disabled={deleteSensor.isPending}
              className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700 disabled:opacity-50"
            >
              Delete
            </button>
          </div>
        </div>
      </div>

      {/* Delete Confirmation Modal */}
      {showDeleteConfirm && (
        <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
          <div className="bg-white rounded-lg p-6 max-w-md">
            <h3 className="text-xl font-bold mb-4">Confirm Delete</h3>
            <p className="mb-6">
              Are you sure you want to delete sensor{" "}
              <strong>
                {sensor.data?.pack_ref}.{sensor.data?.label}
              </strong>
              ?
            </p>
            <div className="flex justify-end gap-3">
              <button
                onClick={() => setShowDeleteConfirm(false)}
                className="px-4 py-2 bg-gray-200 rounded hover:bg-gray-300"
              >
                Cancel
              </button>
              <button
                onClick={handleDelete}
                className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      )}

      {showConfigureModal && (
        <ConfigureSensorRetentionModal
          sensor={sensor.data}
          isSaving={updateSensor.isPending}
          onClose={() => setShowConfigureModal(false)}
          onSave={async (payload) => {
            await updateSensor.mutateAsync({
              ref: sensorRef,
              data: payload,
            });
            setShowConfigureModal(false);
          }}
        />
      )}

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Main Info Card */}
        <div className="lg:col-span-2 space-y-6">
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-xl font-semibold mb-4">Sensor Information</h2>
            <dl className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div>
                <dt className="text-sm font-medium text-gray-500">Reference</dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {sensor.data?.ref}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Label</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {sensor.data?.label}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Pack</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  <Link
                    to={`/packs/${sensor.data?.pack_ref}`}
                    className="text-blue-600 hover:text-blue-800"
                  >
                    {sensor.data?.pack_ref}
                  </Link>
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">
                  Entry Point
                </dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {sensor.data?.entrypoint}
                </dd>
              </div>
              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-gray-500">
                  Description
                </dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {sensor.data?.description || "No description provided"}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Status</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {sensor.data?.enabled ? "Enabled" : "Disabled"}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">
                  Sensor Ref
                </dt>
                <dd className="mt-1 text-sm text-gray-900 font-mono">
                  {sensor.data?.ref}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Created</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(sensor.data?.created || "").toLocaleString()}
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Updated</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(sensor.data?.updated || "").toLocaleString()}
                </dd>
              </div>
              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-gray-500">
                  Retention Defaults
                </dt>
                <dd className="mt-1 flex flex-wrap gap-2">
                  <span className="text-xs px-2 py-1 rounded bg-slate-50 text-slate-700">
                    Logs:{" "}
                    {formatRetention(
                      sensor.data?.log_retention_policy as
                        | RetentionPolicy
                        | null
                        | undefined,
                      sensor.data?.log_retention_limit,
                      "system default",
                    )}
                  </span>
                  <span className="text-xs px-2 py-1 rounded bg-teal-50 text-teal-700">
                    Non-log artifacts:{" "}
                    {formatRetention(
                      sensor.data?.artifact_retention_policy as
                        | RetentionPolicy
                        | null
                        | undefined,
                      sensor.data?.artifact_retention_limit,
                      "system default",
                    )}
                  </span>
                </dd>
              </div>
            </dl>
          </div>

          <div className="bg-white shadow rounded-lg p-6">
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-xl font-semibold">Sensor Logs</h2>
              <label className="inline-flex items-center gap-2 text-sm text-gray-700">
                <input
                  type="checkbox"
                  checked={followLogs}
                  onChange={(event) => setFollowLogs(event.target.checked)}
                  className="rounded border-gray-300"
                />
                Follow
              </label>
            </div>
            <div className="flex gap-2 mb-3">
              {(["stderr", "stdout"] as const).map((stream) => {
                const entry = logSummary?.logs.find(
                  (log) => log.stream === stream,
                );
                return (
                  <button
                    key={stream}
                    onClick={() => setLogStream(stream)}
                    disabled={!entry?.artifact_id}
                    className={`px-3 py-1 rounded text-sm disabled:cursor-not-allowed disabled:bg-gray-50 disabled:text-gray-400 ${
                      logStream === stream
                        ? "bg-blue-600 text-white disabled:bg-gray-100 disabled:text-gray-500"
                        : "bg-gray-100 text-gray-700 hover:bg-gray-200"
                    }`}
                  >
                    {stream}
                    {!entry?.artifact_id && " (not created)"}
                  </button>
                );
              })}
            </div>
            <pre className="bg-gray-950 text-gray-100 rounded p-4 overflow-auto max-h-96 text-xs whitespace-pre-wrap">
              {!selectedLogAvailable
                ? `${logStream} log has not been created yet.`
                : logLoading
                  ? "Loading log tail..."
                  : logContent || "No log output available"}
            </pre>
          </div>
        </div>

        {/* Sidebar */}
        <div className="space-y-6">
          {/* Quick Actions */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Quick Actions</h2>
            <div className="space-y-2">
              <Link
                to={`/packs/${sensor.data?.pack_ref}`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Pack
              </Link>
              <Link
                to={`/triggers`}
                className="block w-full px-4 py-2 text-sm text-center bg-gray-100 hover:bg-gray-200 rounded"
              >
                View Triggers
              </Link>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function ConfigureSensorRetentionModal({
  sensor,
  isSaving,
  onClose,
  onSave,
}: {
  sensor?: SensorResponse;
  isSaving: boolean;
  onClose: () => void;
  onSave: (payload: UpdateSensorRequest) => Promise<void>;
}) {
  const [logRetention, setLogRetention] = useState<{
    policy: RetentionPolicy | null;
    limit: number | null;
  }>({
    policy: (sensor?.log_retention_policy as RetentionPolicy | undefined) ?? null,
    limit: sensor?.log_retention_limit ?? null,
  });
  const [artifactRetention, setArtifactRetention] = useState<{
    policy: RetentionPolicy | null;
    limit: number | null;
  }>({
    policy:
      (sensor?.artifact_retention_policy as RetentionPolicy | undefined) ??
      null,
    limit: sensor?.artifact_retention_limit ?? null,
  });
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setError(null);
    try {
      await onSave({
        log_retention_policy: logRetention.policy
          ? {
              op: LogRetentionPolicyPatch.op.SET,
              value: logRetention.policy as RetentionPolicyType,
            }
          : ({
              op: "clear",
            } as unknown as UpdateSensorRequest["log_retention_policy"]),
        log_retention_limit: logRetention.limit
          ? { op: LogRetentionLimitPatch.op.SET, value: logRetention.limit }
          : ({
              op: "clear",
            } as unknown as UpdateSensorRequest["log_retention_limit"]),
        artifact_retention_policy: artifactRetention.policy
          ? {
              op: LogRetentionPolicyPatch.op.SET,
              value: artifactRetention.policy as RetentionPolicyType,
            }
          : ({
              op: "clear",
            } as unknown as UpdateSensorRequest["artifact_retention_policy"]),
        artifact_retention_limit: artifactRetention.limit
          ? {
              op: LogRetentionLimitPatch.op.SET,
              value: artifactRetention.limit,
            }
          : ({
              op: "clear",
            } as unknown as UpdateSensorRequest["artifact_retention_limit"]),
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save retention");
    }
  };

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
      <div className="bg-white rounded-lg shadow-xl w-full max-w-xl">
        <div className="flex items-center justify-between px-6 py-4 border-b">
          <h2 className="text-xl font-bold">Configure Sensor Retention</h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
            <X className="h-5 w-5" />
          </button>
        </div>
        <div className="px-6 py-4 space-y-3">
          <RetentionPolicyControls
            title="Sensor logs"
            description="Default retention for stdout/stderr log artifacts registered by this sensor."
            policy={logRetention.policy}
            limit={logRetention.limit}
            onChange={setLogRetention}
          />
          <RetentionPolicyControls
            title="Non-log artifacts"
            description="Default retention for non-log artifacts associated with this sensor."
            policy={artifactRetention.policy}
            limit={artifactRetention.limit}
            onChange={setArtifactRetention}
          />
          {error && (
            <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
              {error}
            </div>
          )}
        </div>
        <div className="flex justify-end gap-3 px-6 py-4 border-t">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-2 text-sm text-gray-700 bg-gray-100 rounded hover:bg-gray-200"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={submit}
            disabled={isSaving}
            className="px-4 py-2 text-sm text-white bg-blue-600 rounded hover:bg-blue-700 disabled:opacity-50"
          >
            {isSaving ? "Saving..." : "Save Retention"}
          </button>
        </div>
      </div>
    </div>
  );
}
