/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { NullableStringPatch } from "./NullableStringPatch";
import type { WorkQueueBatchMode } from "./WorkQueueBatchMode";
import type { WorkQueueUpdateStrategy } from "./WorkQueueUpdateStrategy";
export type UpdateWorkQueueRequest = {
  accepting_new_items?: boolean | null;
  action_params?: Record<string, any> | null;
  allow_pending_update?: boolean | null;
  batch_mode?: null | WorkQueueBatchMode;
  config?: Record<string, any> | null;
  default_priority?: number | null;
  description?: null | NullableStringPatch;
  dispatch_action_ref?: string | null;
  enabled?: boolean | null;
  item_schema?: Record<string, any> | null;
  label?: string | null;
  pack_ref?: null | NullableStringPatch;
  /**
   * Permission set refs to apply to executions dispatched by this queue. Omit
   * to keep the current value. Provide null to inherit the dispatch action
   * default, or an empty array to force no API token.
   */
  permission_set_refs?: Array<string> | null;
  /**
   * Replace the restricted visibility allow-list.
   */
  reference_allowed_pack_refs?: Array<string> | null;
  reference_visibility?: null | ActionReferenceVisibility;
  /**
   * Optional template used to resolve execution trace tags for queue dispatches.
   * Omit to keep current value. Provide null to clear.
   */
  trace_tag_template?: string | null;
  update_strategy?: null | WorkQueueUpdateStrategy;
};
