import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ApiError,
  InquiriesService,
  type InquiryRespondRequest,
  type InquiryStatus,
} from "@/api";

export interface InquiryListFilters {
  status?: InquiryStatus;
  createdByExecution?: number;
  assignedTo?: number;
  workflowActionRef?: string;
  workflowPackRef?: string;
  offset: number;
  limit: number;
}

export const inquiryKeys = {
  all: ["inquiries"] as const,
  lists: () => [...inquiryKeys.all, "list"] as const,
  list: (filters: InquiryListFilters) =>
    [
      ...inquiryKeys.lists(),
      filters.status ?? null,
      filters.createdByExecution ?? null,
      filters.assignedTo ?? null,
      filters.workflowActionRef ?? null,
      filters.workflowPackRef ?? null,
      filters.offset,
      filters.limit,
    ] as const,
  details: () => [...inquiryKeys.all, "detail"] as const,
  detail: (id: number) => [...inquiryKeys.details(), id] as const,
  respond: (id: number) => [...inquiryKeys.all, "respond", id] as const,
};

export function useInquiries(filters: InquiryListFilters) {
  return useQuery({
    queryKey: inquiryKeys.list(filters),
    queryFn: () => InquiriesService.listInquiries(filters),
  });
}

export function useInquiry(id: number, enabled = true) {
  return useQuery({
    queryKey: inquiryKeys.detail(id),
    queryFn: () => InquiriesService.getInquiry({ id }),
    enabled,
  });
}

export function useRespondToInquiry(id: number) {
  const queryClient = useQueryClient();

  const refreshInquiry = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: inquiryKeys.detail(id) }),
      queryClient.invalidateQueries({ queryKey: inquiryKeys.lists() }),
    ]);
  };

  return useMutation({
    mutationKey: inquiryKeys.respond(id),
    mutationFn: (requestBody: InquiryRespondRequest) =>
      InquiriesService.respondToInquiry({ id, requestBody }),
    onSuccess: refreshInquiry,
    onError: (error) => {
      if (error instanceof ApiError && error.status === 409) {
        return refreshInquiry();
      }
    },
  });
}
