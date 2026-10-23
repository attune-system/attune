import { useQuery } from "@tanstack/react-query";
import { ActionsService } from "@/api";

export function useExecutionOptions(actionRef: string) {
  return useQuery({
    queryKey: ["action-execution-options", actionRef],
    queryFn: () => ActionsService.getExecutionOptions({ ref: actionRef }),
    enabled: actionRef.length > 0,
    staleTime: 0,
    retry: false,
  });
}
