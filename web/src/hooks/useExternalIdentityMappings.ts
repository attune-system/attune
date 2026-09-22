import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ExternalIdentityMappingsService,
  type CreateExternalIdentityMappingRequest,
  type UpdateExternalIdentityMappingRequest,
} from "@/api";

const queryKey = (integrationIdentity: number) => [
  "identities",
  integrationIdentity,
  "external-identity-mappings",
];

export function useExternalIdentityMappings(
  integrationIdentity: number,
  page: number,
) {
  return useQuery({
    queryKey: [...queryKey(integrationIdentity), page],
    queryFn: () =>
      ExternalIdentityMappingsService.listExternalIdentityMappings({
        integrationIdentity,
        page,
        pageSize: 20,
      }),
    enabled: integrationIdentity > 0,
    staleTime: 30000,
  });
}

export function useCreateExternalIdentityMapping() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      integrationIdentity,
      requestBody,
    }: {
      integrationIdentity: number;
      requestBody: CreateExternalIdentityMappingRequest;
    }) =>
      ExternalIdentityMappingsService.createExternalIdentityMapping({
        integrationIdentity,
        requestBody,
      }),
    onSuccess: (_, variables) =>
      queryClient.invalidateQueries({
        queryKey: queryKey(variables.integrationIdentity),
      }),
  });
}

export function useUpdateExternalIdentityMapping() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      integrationIdentity,
      mappingId,
      requestBody,
    }: {
      integrationIdentity: number;
      mappingId: number;
      requestBody: UpdateExternalIdentityMappingRequest;
    }) =>
      ExternalIdentityMappingsService.updateExternalIdentityMapping({
        integrationIdentity,
        mappingId,
        requestBody,
      }),
    onSuccess: (_, variables) =>
      queryClient.invalidateQueries({
        queryKey: queryKey(variables.integrationIdentity),
      }),
  });
}

export function useDeleteExternalIdentityMapping() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      integrationIdentity,
      mappingId,
    }: {
      integrationIdentity: number;
      mappingId: number;
    }) =>
      ExternalIdentityMappingsService.deleteExternalIdentityMapping({
        integrationIdentity,
        mappingId,
      }),
    onSuccess: (_, variables) =>
      queryClient.invalidateQueries({
        queryKey: queryKey(variables.integrationIdentity),
      }),
  });
}
