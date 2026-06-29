/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { ResolvedWorkQueueDispatchTuningResponse } from "./ResolvedWorkQueueDispatchTuningResponse";
import type { WorkQueueBatchMode } from "./WorkQueueBatchMode";
import type { WorkQueueUpdateStrategy } from "./WorkQueueUpdateStrategy";
export type WorkQueueResponse = {
  accepting_new_items: boolean;
  action_params: Record<string, any>;
  allow_pending_update: boolean;
  batch_mode: WorkQueueBatchMode;
  config: Record<string, any>;
  created: string;
  default_priority: number;
  description?: string | null;
  dispatch_action?: null | i64;
  dispatch_action_ref: string;
  trace_tag_template?: string | null;
  enabled: boolean;
  id: i64;
  is_adhoc: boolean;
  item_schema: Record<string, any>;
  label: string;
  pack?: null | i64;
  pack_ref?: string | null;
  permission_set_refs?: any[] | null;
  ref: string;
  reference_allowed_pack_refs?: Array<string>;
  reference_visibility: ActionReferenceVisibility;
  resolved_dispatch_tuning?: null | ResolvedWorkQueueDispatchTuningResponse;
  update_strategy: WorkQueueUpdateStrategy;
  updated: string;
};
