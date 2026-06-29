import { Link, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import QueueForm from "@/components/queues/QueueForm";
import { useQueue } from "@/hooks/useQueues";

export default function QueueEditPage() {
  const { ref } = useParams<{ ref: string }>();
  const queueRef = ref ?? "";
  const { data, isLoading, error } = useQueue(queueRef);
  const queue = data?.data;

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex h-64 items-center justify-center">
          <div className="h-12 w-12 animate-spin rounded-full border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error || !queue) {
    return (
      <div className="mx-auto max-w-4xl p-6">
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-red-700">
          {error instanceof Error ? error.message : "Queue not found"}
        </div>
      </div>
    );
  }

  if (!queue.is_adhoc) {
    return (
      <div className="mx-auto max-w-4xl p-6">
        <div className="rounded-lg border border-yellow-200 bg-yellow-50 px-4 py-3 text-yellow-800">
          This queue is pack-managed and cannot be edited in the UI. Update the
          pack queue definition files instead.
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-6xl p-6">
      <div className="mb-6">
        <Link
          to={`/queues/${encodeURIComponent(queue.ref)}`}
          className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900"
        >
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back to Queue
        </Link>
        <h1 className="mt-4 text-3xl font-bold text-gray-900">Edit Queue</h1>
        <p className="mt-2 text-gray-600">
          Update the API-managed queue configuration for{" "}
          <span className="font-mono text-gray-800">{queue.ref}</span>.
        </p>
      </div>

      <QueueForm initialData={queue} isEditing />
    </div>
  );
}
