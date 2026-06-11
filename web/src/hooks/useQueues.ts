import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  WorkQueuesService,
  type ApplyWorkQueueItemsRequest,
  type CreateWorkQueueRequest,
  type EnqueueWorkQueueItemRequest,
  type ListQueueItemsParams,
  type ListQueuesParams,
  type PreviewWorkQueueItemsRequest,
  type UpdateWorkQueueItemRequest,
  type UpdateWorkQueueRequest,
} from "@/api/queues";

const queueKeys = {
  all: ["queues"] as const,
  lists: () => [...queueKeys.all, "list"] as const,
  list: (params?: ListQueuesParams) => [...queueKeys.lists(), params] as const,
  details: () => [...queueKeys.all, "detail"] as const,
  detail: (ref: string) => [...queueKeys.details(), ref] as const,
  items: (ref: string) => [...queueKeys.detail(ref), "items"] as const,
  itemList: (ref: string, params?: ListQueueItemsParams) =>
    [...queueKeys.items(ref), params] as const,
};

export function useQueues(params?: ListQueuesParams) {
  return useQuery({
    queryKey: queueKeys.list(params),
    queryFn: () => WorkQueuesService.listQueues(params),
    staleTime: 30000,
  });
}

export function useQueue(ref: string) {
  return useQuery({
    queryKey: queueKeys.detail(ref),
    queryFn: () => WorkQueuesService.getQueue({ ref }),
    enabled: !!ref,
    staleTime: 30000,
  });
}

export function useCreateQueue() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: CreateWorkQueueRequest) =>
      WorkQueuesService.createQueue({ requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queueKeys.all });
    },
  });
}

export function useUpdateQueue() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ ref, data }: { ref: string; data: UpdateWorkQueueRequest }) =>
      WorkQueuesService.updateQueue({ ref, requestBody: data }),
    onSuccess: (_response, variables) => {
      queryClient.invalidateQueries({ queryKey: queueKeys.all });
      queryClient.invalidateQueries({ queryKey: queueKeys.detail(variables.ref) });
    },
  });
}

export function useDeleteQueue() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (ref: string) => WorkQueuesService.deleteQueue({ ref }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queueKeys.all });
    },
  });
}

export function useQueueItems(ref: string, params?: ListQueueItemsParams) {
  return useQuery({
    queryKey: queueKeys.itemList(ref, params),
    queryFn: () => WorkQueuesService.listQueueItems({ ref, ...params }),
    enabled: !!ref,
    staleTime: 15000,
  });
}

export function usePreviewQueueItemsBySelector() {
  return useMutation({
    mutationFn: ({
      ref,
      data,
    }: {
      ref: string;
      data: PreviewWorkQueueItemsRequest;
    }) => WorkQueuesService.previewQueueItemsBySelector({ ref, requestBody: data }),
  });
}

export function useApplyQueueItemsBySelector() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      ref,
      data,
    }: {
      ref: string;
      data: ApplyWorkQueueItemsRequest;
    }) => WorkQueuesService.applyQueueItemsBySelector({ ref, requestBody: data }),
    onSuccess: (_response, variables) => {
      queryClient.invalidateQueries({ queryKey: queueKeys.items(variables.ref) });
      queryClient.invalidateQueries({ queryKey: queueKeys.detail(variables.ref) });
    },
  });
}

export function useEnqueueQueueItem() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ ref, data }: { ref: string; data: EnqueueWorkQueueItemRequest }) =>
      WorkQueuesService.enqueueQueueItem({ ref, requestBody: data }),
    onSuccess: (_response, variables) => {
      queryClient.invalidateQueries({ queryKey: queueKeys.items(variables.ref) });
      queryClient.invalidateQueries({ queryKey: queueKeys.detail(variables.ref) });
    },
  });
}

export function useUpdateQueueItem() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      ref,
      itemId,
      data,
    }: {
      ref: string;
      itemId: number;
      data: UpdateWorkQueueItemRequest;
    }) => WorkQueuesService.updateQueueItem({ ref, itemId, requestBody: data }),
    onSuccess: (_response, variables) => {
      queryClient.invalidateQueries({ queryKey: queueKeys.items(variables.ref) });
      queryClient.invalidateQueries({ queryKey: queueKeys.detail(variables.ref) });
    },
  });
}

export function useDeleteQueueItem() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ ref, itemId }: { ref: string; itemId: number }) =>
      WorkQueuesService.deleteQueueItem({ ref, itemId }),
    onSuccess: (_response, variables) => {
      queryClient.invalidateQueries({ queryKey: queueKeys.items(variables.ref) });
      queryClient.invalidateQueries({ queryKey: queueKeys.detail(variables.ref) });
    },
  });
}

export { queueKeys };
