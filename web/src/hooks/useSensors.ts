import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { SensorsService } from "@/api";
import { OpenAPI } from "@/api/core/OpenAPI";
import { request as apiRequest } from "@/api/core/request";
import type {
  CreateSensorRequest,
  PaginatedResponse_SensorSummary,
  UpdateSensorRequest,
} from "@/api";

interface SensorsQueryParams {
  page?: number;
  pageSize?: number;
  packRef?: string;
  enabled?: boolean;
}

interface InfiniteSensorsQueryParams
  extends Omit<SensorsQueryParams, "page" | "pageSize"> {
  query?: string;
}

// Fetch one page of sensors.
export function useSensors(params?: SensorsQueryParams) {
  return useQuery({
    queryKey: ["sensors", params],
    queryFn: async () => {
      if (params?.packRef) {
        return await SensorsService.listSensorsByPack({
          packRef: params.packRef,
          page: params?.page || 1,
          pageSize: params?.pageSize || 50,
        });
      }
      return await SensorsService.listSensors({
        page: params?.page || 1,
        pageSize: params?.pageSize || 50,
      });
    },
    staleTime: 30000, // 30 seconds
  });
}

// Fetch sensor pages only as the catalog scrolls.
export function useInfiniteSensors(params?: InfiniteSensorsQueryParams) {
  return useInfiniteQuery({
    queryKey: ["sensors", "infinite", params],
    initialPageParam: 1,
    queryFn: ({ pageParam }) => {
      return apiRequest<PaginatedResponse_SensorSummary>(OpenAPI, {
        method: "GET",
        url: params?.packRef
          ? "/api/v1/packs/{pack_ref}/sensors"
          : "/api/v1/sensors",
        path: params?.packRef ? { pack_ref: params.packRef } : undefined,
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

// Fetch enabled sensors only
export function useEnabledSensors(
  params?: Omit<SensorsQueryParams, "enabled">,
) {
  return useSensors({ ...params, enabled: true });
}

// Fetch single sensor by reference
export function useSensor(ref: string) {
  return useQuery({
    queryKey: ["sensors", ref],
    queryFn: async () => {
      return await SensorsService.getSensor({ ref });
    },
    enabled: !!ref,
    staleTime: 30000,
  });
}

// Fetch sensors by pack
export function usePackSensors(packRef: string) {
  return useQuery({
    queryKey: ["packs", packRef, "sensors"],
    queryFn: async () => {
      return await SensorsService.listSensorsByPack({
        packRef,
        page: 1,
        pageSize: 100,
      });
    },
    enabled: !!packRef,
    staleTime: 30000,
  });
}

// Create a new sensor
export function useCreateSensor() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreateSensorRequest) => {
      return await SensorsService.createSensor({ requestBody: data });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["sensors"] });
    },
  });
}

// Update existing sensor
export function useUpdateSensor() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      data,
    }: {
      ref: string;
      data: UpdateSensorRequest;
    }) => {
      return await SensorsService.updateSensor({ ref, requestBody: data });
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["sensors"] });
      queryClient.invalidateQueries({ queryKey: ["sensors", variables.ref] });
    },
  });
}

// Delete sensor
export function useDeleteSensor() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      await SensorsService.deleteSensor({ ref });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["sensors"] });
    },
  });
}
