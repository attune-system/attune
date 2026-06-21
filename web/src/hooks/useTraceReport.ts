import { useQuery } from "@tanstack/react-query";
import { OpenAPI } from "@/api/core/OpenAPI";
import { request as __request } from "@/api/core/request";

export type TraceReportResponse = {
  trace_tag: string;
  origins: string[];
  executions: Array<{
    id: number;
    action_ref: string;
    status: string;
    trace_tag?: string | null;
    rule_ref?: string | null;
    trigger_ref?: string | null;
    created: string;
    updated: string;
  }>;
  enforcements: Array<{
    id: number;
    rule?: number | null;
    rule_ref: string;
    trigger_ref: string;
    event?: number | null;
    status: string;
    condition: string;
    created: string;
    resolved_at?: string | null;
  }>;
  events: Array<{
    id: number;
    trigger_ref: string;
    rule_ref?: string | null;
    created: string;
  }>;
  queue_dispatches: Array<{
    id: number;
    queue_ref: string;
    execution: number;
    status: string;
    leased_item_count: number;
    created: string;
    updated: string;
  }>;
  queue_items: Array<{
    id: number;
    queue_ref: string;
    status: string;
    item_key?: string | null;
    priority: number;
    requested_by_execution?: number | null;
    leased_execution?: number | null;
    created: string;
    updated: string;
  }>;
};

export function useTraceReport(traceTag: string) {
  return useQuery({
    queryKey: ["trace-report", traceTag],
    queryFn: async () => {
      const response = await __request(OpenAPI, {
        method: "GET",
        url: "/api/v1/traces/{trace_tag}",
        path: { trace_tag: traceTag },
      });
      return response as { data: TraceReportResponse };
    },
    enabled: traceTag.trim().length > 0,
    staleTime: 15000,
  });
}
