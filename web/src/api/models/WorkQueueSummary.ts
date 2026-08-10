/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { i64 } from "./i64";
export type WorkQueueSummary = {
  accepting_new_items: boolean;
  created: string;
  description?: string | null;
  dispatch_action_ref: string;
  enabled: boolean;
  id: i64;
  is_adhoc: boolean;
  label: string;
  pack_ref?: string | null;
  ref: string;
  reference_allowed_pack_refs: Array<string>;
  reference_visibility: ActionReferenceVisibility;
  trace_tag_template?: string | null;
  updated: string;
};
