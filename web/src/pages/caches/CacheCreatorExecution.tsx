import { Link } from "react-router-dom";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";

export default function CacheCreatorExecution({
  executionId,
}: {
  executionId: number | null;
}) {
  const { user } = useAuth();
  if (executionId === null) return <span>None recorded</span>;
  return hasPermission(user, "executions", "read") ? (
    <Link
      to={`/executions/${executionId}`}
      className="text-teal-700 hover:underline"
    >
      Execution #{executionId}
    </Link>
  ) : (
    <span>Execution #{executionId}</span>
  );
}
