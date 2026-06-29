import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RetentionService, type RetentionConfig } from "@/api/retention";

export const retentionKeys = {
  all: ["retention-config"] as const,
  detail: () => [...retentionKeys.all, "detail"] as const,
};

export function useRetentionConfig() {
  return useQuery({
    queryKey: retentionKeys.detail(),
    queryFn: () => RetentionService.getRetentionConfig(),
    staleTime: 30000,
  });
}

export function useUpdateRetentionConfig() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (config: RetentionConfig) =>
      RetentionService.updateRetentionConfig({ requestBody: config }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: retentionKeys.all });
    },
  });
}
