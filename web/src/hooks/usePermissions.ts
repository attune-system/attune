import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { PermissionsService } from "@/api";
import { apiClient } from "@/lib/api-client";
import type {
  CreateIdentityRequest,
  UpdateIdentityRequest,
  CreatePermissionAssignmentRequest,
  PermissionSetSummary,
  UpdatePermissionSetRequest,
} from "@/api";

export interface IntegrationToken {
  id: number;
  identity_id: number;
  label: string;
  description?: string | null;
  token_prefix: string;
  token_suffix: string;
  created_by?: number | null;
  expires_at?: string | null;
  last_used_at?: string | null;
  last_used_ip?: string | null;
  revoked_at?: string | null;
  revoked_by?: number | null;
  revocation_reason?: string | null;
  active: boolean;
  created: string;
  updated: string;
}

export interface CreateIntegrationTokenInput {
  label: string;
  description?: string | null;
  expires_at?: string | null;
}

export interface CreateIntegrationTokenResponse {
  token: string;
  integration_token: IntegrationToken;
}

// Fetch all identities with pagination
export function useIdentities(params?: { page?: number; pageSize?: number }) {
  return useQuery({
    queryKey: ["identities", params],
    queryFn: async () => {
      return await PermissionsService.listIdentities({
        page: params?.page || 1,
        pageSize: params?.pageSize || 50,
      });
    },
    staleTime: 30000,
  });
}

// Fetch single identity by ID
export function useIdentity(id: number) {
  return useQuery({
    queryKey: ["identities", id],
    queryFn: async () => {
      return await PermissionsService.getIdentity({ id });
    },
    enabled: id > 0,
    staleTime: 30000,
  });
}

// Create a new identity
export function useCreateIdentity() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreateIdentityRequest) => {
      return await PermissionsService.createIdentity({ requestBody: data });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
    },
  });
}

// Update an existing identity
export function useUpdateIdentity() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      id,
      data,
    }: {
      id: number;
      data: UpdateIdentityRequest;
    }) => {
      return await PermissionsService.updateIdentity({ id, requestBody: data });
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
      queryClient.invalidateQueries({
        queryKey: ["identities", variables.id],
      });
    },
  });
}

// Delete an identity
export function useDeleteIdentity() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (id: number) => {
      return await PermissionsService.deleteIdentity({ id });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
    },
  });
}

// Fetch permission sets
export function usePermissionSets(
  packRef?: string | null,
  options: { enabled?: boolean } = {},
) {
  return useQuery({
    queryKey: ["permission-sets", packRef],
    queryFn: async () => {
      return await PermissionsService.listPermissionSets({ packRef });
    },
    enabled: options.enabled ?? true,
    staleTime: 30000,
  });
}

export function useUpdatePermissionSet() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      id,
      data,
    }: {
      id: number;
      data: UpdatePermissionSetRequest;
    }) => {
      return await PermissionsService.updatePermissionSet({
        id,
        requestBody: data,
      });
    },
    onSuccess: (response) => {
      const updated = response.data as PermissionSetSummary;
      queryClient.invalidateQueries({ queryKey: ["permission-sets"] });
      if (updated?.pack_ref) {
        queryClient.invalidateQueries({
          queryKey: ["permission-sets", updated.pack_ref],
        });
      }
    },
  });
}

// Fetch permission assignments for an identity
export function useIdentityPermissions(id: number) {
  return useQuery({
    queryKey: ["identity-permissions", id],
    queryFn: async () => {
      return await PermissionsService.listIdentityPermissions({ id });
    },
    enabled: id > 0,
    staleTime: 30000,
  });
}

// Create a permission assignment
export function useCreatePermissionAssignment() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreatePermissionAssignmentRequest) => {
      return await PermissionsService.createPermissionAssignment({
        requestBody: data,
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
      queryClient.invalidateQueries({ queryKey: ["identity-permissions"] });
      queryClient.invalidateQueries({ queryKey: ["permission-sets"] });
    },
  });
}

// Delete a permission assignment
export function useDeletePermissionAssignment() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (id: number) => {
      return await PermissionsService.deletePermissionAssignment({ id });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
      queryClient.invalidateQueries({ queryKey: ["identity-permissions"] });
      queryClient.invalidateQueries({ queryKey: ["permission-sets"] });
    },
  });
}

// Create a role assignment for an identity
export function useCreateIdentityRoleAssignment() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      identityId,
      role,
    }: {
      identityId: number;
      role: string;
    }) => {
      return await PermissionsService.createIdentityRoleAssignment({
        id: identityId,
        requestBody: { role },
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
    },
  });
}

// Delete a role assignment from an identity
export function useDeleteIdentityRoleAssignment() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (id: number) => {
      return await PermissionsService.deleteIdentityRoleAssignment({ id });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
    },
  });
}

// Create a role assignment for a permission set
export function useCreatePermissionSetRoleAssignment() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      permissionSetId,
      role,
    }: {
      permissionSetId: number;
      role: string;
    }) => {
      return await PermissionsService.createPermissionSetRoleAssignment({
        id: permissionSetId,
        requestBody: { role },
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["permission-sets"] });
    },
  });
}

// Delete a role assignment from a permission set
export function useDeletePermissionSetRoleAssignment() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (id: number) => {
      return await PermissionsService.deletePermissionSetRoleAssignment({ id });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["permission-sets"] });
    },
  });
}

// Freeze an identity
export function useFreezeIdentity() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (id: number) => {
      return await PermissionsService.freezeIdentity({ id });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
    },
  });
}

// Unfreeze an identity
export function useUnfreezeIdentity() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (id: number) => {
      return await PermissionsService.unfreezeIdentity({ id });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identities"] });
    },
  });
}

export function useIntegrationTokens(identityId: number) {
  return useQuery({
    queryKey: ["identities", identityId, "integration-tokens"],
    queryFn: async () => {
      const response = await apiClient.get<{ data: IntegrationToken[] }>(
        `/api/v1/identities/${identityId}/integration-tokens`,
      );
      return response.data.data;
    },
    enabled: identityId > 0,
    staleTime: 30000,
  });
}

export function useCreateIntegrationToken() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      identityId,
      data,
    }: {
      identityId: number;
      data: CreateIntegrationTokenInput;
    }) => {
      const response = await apiClient.post<{
        data: CreateIntegrationTokenResponse;
      }>(`/api/v1/identities/${identityId}/integration-tokens`, data);
      return response.data.data;
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["identities", variables.identityId, "integration-tokens"],
      });
    },
  });
}

export function useRevokeIntegrationToken() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      identityId,
      tokenId,
      reason,
    }: {
      identityId: number;
      tokenId: number;
      reason?: string;
    }) => {
      const response = await apiClient.post<{ data: IntegrationToken }>(
        `/api/v1/identities/${identityId}/integration-tokens/${tokenId}/revoke`,
        { reason },
      );
      return response.data.data;
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["identities", variables.identityId, "integration-tokens"],
      });
    },
  });
}

export function useDeleteIntegrationToken() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      identityId,
      tokenId,
    }: {
      identityId: number;
      tokenId: number;
    }) => {
      await apiClient.delete(
        `/api/v1/identities/${identityId}/integration-tokens/${tokenId}`,
      );
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["identities", variables.identityId, "integration-tokens"],
      });
    },
  });
}
