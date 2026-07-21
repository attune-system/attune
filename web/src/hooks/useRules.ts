import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { RulesService } from "@/api";
import { OpenAPI } from "@/api/core/OpenAPI";
import { request as apiRequest } from "@/api/core/request";
import type {
  CreateRuleRequest,
  PaginatedResponse_RuleSummary,
  UpdateRuleRequest,
} from "@/api";

interface RulesQueryParams {
  page?: number;
  pageSize?: number;
  packRef?: string;
  actionRef?: string;
  triggerRef?: string;
  enabled?: boolean;
}

interface InfiniteRulesQueryParams
  extends Omit<RulesQueryParams, "page" | "pageSize"> {
  query?: string;
}

// Fetch one page of rules.
export function useRules(params?: RulesQueryParams) {
  return useQuery({
    queryKey: ["rules", params],
    queryFn: async () => {
      // Use specialized endpoints when filtering by pack/action/trigger
      if (params?.packRef) {
        return await RulesService.listRulesByPack({
          packRef: params.packRef,
          page: params.page || 1,
          pageSize: params.pageSize || 50,
        });
      }
      if (params?.actionRef) {
        return await RulesService.listRulesByAction({
          actionRef: params.actionRef,
          page: params.page || 1,
          pageSize: params.pageSize || 50,
        });
      }
      if (params?.triggerRef) {
        return await RulesService.listRulesByTrigger({
          triggerRef: params.triggerRef,
          page: params.page || 1,
          pageSize: params.pageSize || 50,
        });
      }
      // Default: list all rules
      const response = await RulesService.listRules({
        page: params?.page || 1,
        pageSize: params?.pageSize || 50,
      });
      return response;
    },
    staleTime: 30000, // 30 seconds
  });
}

// Fetch rule pages only as the catalog scrolls.
export function useInfiniteRules(params?: InfiniteRulesQueryParams) {
  return useInfiniteQuery({
    queryKey: ["rules", "infinite", params],
    initialPageParam: 1,
    queryFn: ({ pageParam }) => {
      return apiRequest<PaginatedResponse_RuleSummary>(OpenAPI, {
        method: "GET",
        url: params?.packRef
          ? "/api/v1/packs/{pack_ref}/rules"
          : params?.actionRef
            ? "/api/v1/actions/{action_ref}/rules"
            : params?.triggerRef
              ? "/api/v1/triggers/{trigger_ref}/rules"
              : "/api/v1/rules",
        path: params?.packRef
          ? { pack_ref: params.packRef }
          : params?.actionRef
            ? { action_ref: params.actionRef }
            : params?.triggerRef
              ? { trigger_ref: params.triggerRef }
              : undefined,
        query: {
          page: pageParam,
          page_size: 50,
          q: params?.query || undefined,
        },
      });
    },
    getNextPageParam: (lastPage) =>
      lastPage.pagination.has_next
        ? lastPage.pagination.page + 1
        : undefined,
    staleTime: 30000, // 30 seconds
  });
}

// Fetch enabled rules only
export function useEnabledRules(params?: Omit<RulesQueryParams, "enabled">) {
  return useRules({ ...params, enabled: true });
}

// Fetch single rule by ref
export function useRule(ref: string) {
  return useQuery({
    queryKey: ["rules", ref],
    queryFn: async () => {
      const response = await RulesService.getRule({ ref });
      return response;
    },
    enabled: !!ref,
    staleTime: 30000,
  });
}

// Fetch rules by pack
export function usePackRules(packRef: string) {
  return useQuery({
    queryKey: ["packs", packRef, "rules"],
    queryFn: () =>
      RulesService.listRulesByPack({ packRef, page: 1, pageSize: 50 }),
    enabled: !!packRef,
    staleTime: 30000,
  });
}

// Create a new rule
export function useCreateRule() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreateRuleRequest) => {
      const response = await RulesService.createRule({ requestBody: data });
      return response;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
    },
  });
}

// Update existing rule
export function useUpdateRule() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      data,
    }: {
      ref: string;
      data: UpdateRuleRequest;
    }) => {
      const response = await RulesService.updateRule({
        ref,
        requestBody: data,
      });
      return response;
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
      queryClient.invalidateQueries({ queryKey: ["rules", variables.ref] });
    },
  });
}

// Delete rule
export function useDeleteRule() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      await RulesService.deleteRule({ ref });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
    },
  });
}

// Enable rule
export function useEnableRule() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      const response = await RulesService.enableRule({ ref });
      return response;
    },
    onSuccess: (_, ref) => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
      queryClient.invalidateQueries({ queryKey: ["rules", ref] });
    },
  });
}

// Disable rule
export function useDisableRule() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      const response = await RulesService.disableRule({ ref });
      return response;
    },
    onSuccess: (_, ref) => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
      queryClient.invalidateQueries({ queryKey: ["rules", ref] });
    },
  });
}
