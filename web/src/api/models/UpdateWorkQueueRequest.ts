/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { NullableStringPatch } from './NullableStringPatch';
import type { WorkQueueBatchMode } from './WorkQueueBatchMode';
import type { WorkQueueUpdateStrategy } from './WorkQueueUpdateStrategy';
import type { ActionReferenceVisibility } from './ActionReferenceVisibility';
export type UpdateWorkQueueRequest = {
    accepting_new_items?: boolean | null;
    action_params: any | null;
    allow_pending_update?: boolean | null;
    batch_mode?: (null | WorkQueueBatchMode);
    config: any | null;
    default_priority?: number | null;
    description?: (null | NullableStringPatch);
    dispatch_action_ref?: string | null;
    enabled?: boolean | null;
    item_schema: any | null;
    label?: string | null;
    pack_ref?: (null | NullableStringPatch);
    /**
     * Permission set refs to apply to executions dispatched by this queue. Omit
     * to keep the current value. Provide null to inherit the dispatch action
     * default, or an empty array to force no API token.
     */
    permission_set_refs?: any[] | null;
    reference_allowed_pack_refs?: any[] | null;
    reference_visibility?: (null | ActionReferenceVisibility);
    update_strategy?: (null | WorkQueueUpdateStrategy);
};
