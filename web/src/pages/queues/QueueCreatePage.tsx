import { Link } from "react-router-dom";
import { ArrowLeft, Info } from "lucide-react";
import QueueForm from "@/components/queues/QueueForm";

export default function QueueCreatePage() {
  return (
    <div className="mx-auto max-w-6xl p-6">
      <div className="mb-6">
        <Link
          to="/queues"
          className="inline-flex items-center text-sm text-gray-600 hover:text-gray-900"
        >
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back to Queues
        </Link>
        <h1 className="mt-4 text-3xl font-bold text-gray-900">Create Queue</h1>
        <p className="mt-2 text-gray-600">
          Create an API-managed work queue for manual intake, external API
          producers, or workflow-controlled dispatch.
        </p>
      </div>

      <div className="mb-6 rounded-lg border border-blue-200 bg-blue-50 p-5">
        <div className="flex items-start gap-3">
          <Info className="mt-0.5 h-5 w-5 flex-shrink-0 text-blue-600" />
          <div className="text-sm text-blue-900">
            <h2 className="font-semibold">API-managed queue guidance</h2>
            <ul className="mt-2 list-disc space-y-1 pl-5 text-blue-800">
              <li>Queues created here are editable in the UI.</li>
              <li>
                Pack-managed queue definitions remain view-only and must be
                changed in pack files.
              </li>
              <li>
                Pending queue items can be added and updated from the queue
                detail page.
              </li>
            </ul>
          </div>
        </div>
      </div>

      <QueueForm />
    </div>
  );
}
