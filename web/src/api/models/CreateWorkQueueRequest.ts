/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { WorkQueueBatchMode } from "./WorkQueueBatchMode";
import type { WorkQueueUpdateStrategy } from "./WorkQueueUpdateStrategy";
export type CreateWorkQueueRequest = {
  accepting_new_items?: boolean;
  action_params?: Record<string, any>;
  allow_pending_update?: boolean;
  batch_mode?: WorkQueueBatchMode;
  config?: Record<string, any>;
  default_priority?: number;
  description?: string | null;
  dispatch_action_ref: string;
  enabled?: boolean;
  item_schema?: Record<string, any>;
  label: string;
  pack_ref?: string | null;
  /**
   * Permission set refs to apply to executions dispatched by this queue. Omit
   * to inherit the dispatch action default. Provide an empty array to force no
   * API token.
   */
  permission_set_refs?: any[] | null;
  ref: string;
  /**
   * Pack refs allowed to target this queue when visibility is restricted.
   */
  reference_allowed_pack_refs?: Array<string>;
  reference_visibility?: null | ActionReferenceVisibility;
  /**
   * Optional template used to resolve execution trace tags for queue dispatches.
   */
  trace_tag_template?: string | null;
  update_strategy?: WorkQueueUpdateStrategy;
};
