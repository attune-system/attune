import type { NativeMaintenanceStatus } from "@/api/retention";

function timestamp(value: string | null | undefined): string {
  return value == null ? "None recorded" : value;
}

export function NativeMaintenanceStatusPanel({
  status,
}: {
  status: NativeMaintenanceStatus;
}) {
  return (
    <section className="rounded-lg border border-gray-200 bg-white p-6 shadow-sm space-y-4">
      <h2 className="text-lg font-semibold text-gray-900">
        Native maintenance status
      </h2>
      <p className="text-sm text-gray-600">
        Maintenance is {status.enabled ? "enabled" : "disabled"}. Observed at{" "}
        {status.observed_at}. Status refreshes every 30 seconds. Coverage bounds
        are extrema, not proof of uninterrupted coverage. Dirty and uncovered
        hours use raw reads.
      </p>
      <div className="overflow-x-auto">
        <table className="min-w-full text-left text-sm">
          <caption className="text-left font-medium py-2">
            Partitions and DEFAULT backlog
          </caption>
          <thead className="bg-gray-50 text-gray-600">
            <tr>
              <th className="p-2">Parent</th>
              <th className="p-2">Registered</th>
              <th className="p-2">Future</th>
              <th className="p-2">Missing future</th>
              <th className="p-2">DEFAULT rows</th>
              <th className="p-2">Oldest DEFAULT day</th>
            </tr>
          </thead>
          <tbody>
            {status.partitions.map((partition) => (
              <tr key={partition.parent} className="border-t border-gray-100">
                <td className="p-2">{partition.parent}</td>
                <td className="p-2">{partition.registered_partitions}</td>
                <td className="p-2">{partition.future_partitions}</td>
                <td
                  className={`p-2 ${partition.missing_future_partitions > 0 ? "text-amber-800 font-medium" : ""}`}
                >
                  {partition.missing_future_partitions}
                </td>
                <td className="p-2">
                  {partition.default_count_exact ? "" : "At least "}
                  {partition.default_rows_at_least}
                </td>
                <td className="p-2">
                  {timestamp(partition.oldest_default_day)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="overflow-x-auto">
        <table className="min-w-full text-left text-sm">
          <caption className="text-left font-medium py-2">
            Hourly summaries
          </caption>
          <thead className="bg-gray-50 text-gray-600">
            <tr>
              <th className="p-2">Kind</th>
              <th className="p-2">Covered hours</th>
              <th className="p-2">Coverage bounds</th>
              <th className="p-2">Dirty hours</th>
              <th className="p-2">Notifications</th>
              <th className="p-2">Oldest dirty bucket</th>
              <th className="p-2">Oldest notification</th>
              <th className="p-2">Latest refresh</th>
            </tr>
          </thead>
          <tbody>
            {status.summaries.map((summary) => (
              <tr key={summary.kind} className="border-t border-gray-100">
                <td className="p-2">{summary.kind}</td>
                <td className="p-2">{summary.coverage_hours}</td>
                <td className="p-2">
                  {timestamp(summary.covered_since)} to{" "}
                  {timestamp(summary.covered_until)}
                </td>
                <td className="p-2">{summary.dirty_hours}</td>
                <td className="p-2">{summary.dirty_notifications}</td>
                <td className="p-2">
                  {timestamp(summary.oldest_dirty_bucket)}
                </td>
                <td className="p-2">
                  {timestamp(summary.oldest_notification)}
                </td>
                <td className="p-2">{timestamp(summary.latest_success)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="overflow-x-auto">
        <table className="min-w-full text-left text-sm">
          <caption className="text-left font-medium py-2">Job schedule</caption>
          <thead className="bg-gray-50 text-gray-600">
            <tr>
              <th className="p-2">Job</th>
              <th className="p-2">Next due</th>
              <th className="p-2">Last success</th>
            </tr>
          </thead>
          <tbody>
            {status.schedule.map((job) => (
              <tr key={job.job} className="border-t border-gray-100">
                <td className="p-2">{job.job}</td>
                <td className="p-2">{job.next_due}</td>
                <td className="p-2">{timestamp(job.last_success)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
