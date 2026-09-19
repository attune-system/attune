import { useQuery } from "@tanstack/react-query";
import { ExecutionsService, WorkflowTaskWaitState } from "@/api";

type WorkflowTaskWaitsResponse = Awaited<
  ReturnType<typeof ExecutionsService.listWorkflowTaskWaits>
>;

export type WorkflowTaskWait = WorkflowTaskWaitsResponse["data"][number];

function isExecutionActive(status: string): boolean {
  return [
    "requested",
    "scheduling",
    "scheduled",
    "running",
    "canceling",
  ].includes(status);
}

export function shouldPollWorkflowTaskWaits(
  parentStatus: string,
  waits: WorkflowTaskWait[] | undefined,
): boolean {
  return (
    isExecutionActive(parentStatus) ||
    waits?.some(
      (wait) =>
        wait.state === WorkflowTaskWaitState.WAITING ||
        wait.state === WorkflowTaskWaitState.RELEASED,
    ) === true
  );
}

export function useWorkflowTaskWaits(
  executionId: number | undefined,
  parentStatus: string,
) {
  return useQuery({
    queryKey: ["executions", executionId, "workflow-task-waits"],
    queryFn: () =>
      ExecutionsService.listWorkflowTaskWaits({ id: executionId! }),
    enabled: executionId != null && executionId > 0,
    staleTime: 3000,
    refetchInterval: (query) =>
      shouldPollWorkflowTaskWaits(parentStatus, query.state.data?.data)
        ? 3000
        : false,
  });
}
