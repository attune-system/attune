import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  PoliciesService,
  type CreatePolicyRequest,
  type PolicyScope,
  type UpdatePolicyRequest,
} from "@/api/policies";

export const policyKeys = {
  all: ["policies"] as const,
  list: (params: PolicyQueryParams) => [...policyKeys.all, params] as const,
};

export interface PolicyQueryParams {
  scope?: PolicyScope;
  packRef?: string;
  actionRef?: string;
}

export function usePolicies(params: PolicyQueryParams = {}) {
  return useQuery({
    queryKey: policyKeys.list(params),
    queryFn: () =>
      PoliciesService.listPolicies({
        page: 1,
        pageSize: 100,
        scope: params.scope,
        packRef: params.packRef || undefined,
        actionRef: params.actionRef || undefined,
      }),
    staleTime: 30000,
  });
}

export function useCreatePolicy() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: CreatePolicyRequest) =>
      PoliciesService.createPolicy({ requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: policyKeys.all });
    },
  });
}

export function useUpdatePolicy() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ref, data }: { ref: string; data: UpdatePolicyRequest }) =>
      PoliciesService.updatePolicy({ ref, requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: policyKeys.all });
    },
  });
}

export function useDeletePolicy() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (ref: string) => PoliciesService.deletePolicy({ ref }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: policyKeys.all });
    },
  });
}
