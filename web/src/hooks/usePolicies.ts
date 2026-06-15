import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  PoliciesService,
  type CreatePolicyRequest,
  type ListPoliciesParams,
  type UpdatePolicyRequest,
} from "@/api/policies";

const policyKeys = {
  all: ["policies"] as const,
  lists: () => [...policyKeys.all, "list"] as const,
  list: (params?: ListPoliciesParams) => [...policyKeys.lists(), params] as const,
  details: () => [...policyKeys.all, "detail"] as const,
  detail: (ref: string) => [...policyKeys.details(), ref] as const,
  pack: (packRef: string) => [...policyKeys.lists(), "pack", packRef] as const,
  action: (actionRef: string) => [...policyKeys.lists(), "action", actionRef] as const,
};

export function usePolicies(params?: ListPoliciesParams) {
  return useQuery({
    queryKey: policyKeys.list(params),
    queryFn: () => PoliciesService.listPolicies(params),
    staleTime: 30000,
  });
}

export function usePolicy(ref: string) {
  return useQuery({
    queryKey: policyKeys.detail(ref),
    queryFn: () => PoliciesService.getPolicy({ ref }),
    enabled: !!ref,
    staleTime: 30000,
  });
}

export function usePoliciesByPack(packRef: string) {
  return useQuery({
    queryKey: policyKeys.pack(packRef),
    queryFn: () => PoliciesService.listPoliciesByPack({ packRef }),
    enabled: !!packRef,
    staleTime: 30000,
  });
}

export function usePoliciesByAction(actionRef: string) {
  return useQuery({
    queryKey: policyKeys.action(actionRef),
    queryFn: () => PoliciesService.listPoliciesByAction({ actionRef }),
    enabled: !!actionRef,
    staleTime: 30000,
  });
}

export function useCreatePolicy() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: CreatePolicyRequest) =>
      PoliciesService.createPolicy({ requestBody: data }),
    onSuccess: (response) => {
      queryClient.invalidateQueries({ queryKey: policyKeys.all });
      queryClient.invalidateQueries({
        queryKey: policyKeys.detail(response.data.ref),
      });
    },
  });
}

export function useUpdatePolicy() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ ref, data }: { ref: string; data: UpdatePolicyRequest }) =>
      PoliciesService.updatePolicy({ ref, requestBody: data }),
    onSuccess: (response, variables) => {
      queryClient.invalidateQueries({ queryKey: policyKeys.all });
      queryClient.invalidateQueries({ queryKey: policyKeys.detail(variables.ref) });
      queryClient.invalidateQueries({ queryKey: policyKeys.detail(response.data.ref) });
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

export { policyKeys };
