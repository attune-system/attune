import { Link } from "react-router-dom";
import type { EventResponse } from "@/api";

export default function EventExecutionEmitterCard({
  emitter,
}: {
  emitter: EventResponse["execution_emitter"];
}) {
  if (!emitter) return null;

  return (
    <section
      className="bg-white rounded-lg shadow"
      aria-label="Execution emitter"
    >
      <div className="px-6 py-4 border-b border-gray-200">
        <h2 className="text-lg font-semibold text-gray-900">
          Execution emitter
        </h2>
      </div>
      <div className="px-6 py-4">
        <dl className="space-y-3 text-sm">
          <div>
            <dt className="text-gray-500">Execution</dt>
            <dd>
              <Link
                to={`/executions/${emitter.execution_id}`}
                className="text-blue-600 hover:text-blue-800"
              >
                #{emitter.execution_id}
              </Link>
            </dd>
          </div>
          {[
            ["Action", emitter.action_ref],
            ["Pack", emitter.pack_ref],
            ["Runtime sponsor ID", emitter.sponsor_identity_id],
            ["Admission ID", emitter.admission_id],
            ["Policy revision ID", emitter.policy_revision_id],
            ["Release ID", emitter.release_id],
            ["Release digest", emitter.release_digest],
            ["Attempt", emitter.attempt],
          ].map(([label, value]) => (
            <div key={label}>
              <dt className="text-gray-500">{label}</dt>
              <dd className="text-gray-900 font-mono break-all">{value}</dd>
            </div>
          ))}
        </dl>
        <p className="mt-4 text-xs text-gray-500">
          Recorded when the event was emitted. The execution may no longer be
          retained. The runtime sponsor is separate from the requester and
          sensor source.
        </p>
      </div>
    </section>
  );
}
