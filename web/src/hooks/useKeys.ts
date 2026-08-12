import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { SecretsService, OwnerType } from "@/api";
import type { CreateKeyRequest, UpdateKeyRequest } from "@/api";

interface KeysQueryParams {
  page?: number;
  pageSize?: number;
  ownerType?: OwnerType | null;
  owner?: string | null;
}

interface KeyQueryOptions {
  decrypt?: boolean;
  enabled?: boolean;
}

// Fetch all keys with pagination and filters (values redacted in list)
export function useKeys(params?: KeysQueryParams) {
  return useQuery({
    queryKey: ["keys", params],
    queryFn: async () => {
      return await SecretsService.listKeys({
        page: params?.page || 1,
        perPage: params?.pageSize || 50,
        ownerType: params?.ownerType,
        owner: params?.owner,
      });
    },
    staleTime: 30000, // 30 seconds
  });
}

// Fetch one key. Decryption is opt-in and can be gated on deliberate UI action.
export function useKey(ref: string, options?: KeyQueryOptions) {
  const decrypt = options?.decrypt ?? false;

  return useQuery({
    queryKey: ["keys", ref, { decrypt }],
    queryFn: async () => {
      return await SecretsService.getKey({ ref, decrypt });
    },
    enabled: !!ref && (options?.enabled ?? true),
    staleTime: decrypt ? 0 : 30000,
    gcTime: decrypt ? 0 : undefined,
  });
}

// Create a new key
export function useCreateKey() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreateKeyRequest) => {
      return await SecretsService.createKey({ requestBody: data });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["keys"] });
    },
  });
}

// Update an existing key
export function useUpdateKey() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      data,
    }: {
      ref: string;
      data: UpdateKeyRequest;
    }) => {
      return await SecretsService.updateKey({ ref, requestBody: data });
    },
    onSuccess: (_data, variables) => {
      queryClient.invalidateQueries({ queryKey: ["keys"] });
      queryClient.invalidateQueries({ queryKey: ["keys", variables.ref] });
    },
  });
}

// Delete a key
export function useDeleteKey() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      return await SecretsService.deleteKey({ ref });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["keys"] });
    },
  });
}
