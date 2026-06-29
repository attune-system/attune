import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { AuditService } from "@/api/services/AuditService";

export type ListAuditEventsParams = Parameters<
  typeof AuditService.listAuditEvents
>[0];

export function useAuditEvents(params: ListAuditEventsParams) {
  return useQuery({
    queryKey: ["audit-events", params],
    queryFn: () => AuditService.listAuditEvents(params),
    placeholderData: keepPreviousData,
    staleTime: 15_000,
  });
}

export function useAuditEvent(id: number | null | undefined) {
  return useQuery({
    queryKey: ["audit-event", id],
    queryFn: () => AuditService.getAuditEvent({ id: id! }),
    enabled: id !== null && id !== undefined,
  });
}

export function useAuditEventsByRequest(requestId: string | null | undefined) {
  return useQuery({
    queryKey: ["audit-event-chain", requestId],
    queryFn: () =>
      AuditService.getAuditEventsByRequest({ requestId: requestId! }),
    enabled: !!requestId,
  });
}
