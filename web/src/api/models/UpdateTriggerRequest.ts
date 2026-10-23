/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { TriggerJsonPatch } from "./TriggerJsonPatch";
import type { TriggerStringPatch } from "./TriggerStringPatch";
/**
 * Request DTO for updating a trigger
 */
export type UpdateTriggerRequest = {
  /**
   * Allow a changed live contract despite admitted emitters. Ordinary confirmation is not acknowledgement.
   */
  acknowledge_incompatible_rollout?: boolean;
  description?: null | TriggerStringPatch;
  /**
   * Whether the trigger is enabled
   */
  enabled?: boolean | null;
  event_config_schema?: null | TriggerJsonPatch;
  /**
   * Human-readable label
   */
  label?: string | null;
  out_schema?: null | TriggerJsonPatch;
  param_schema?: null | TriggerJsonPatch;
  /**
   * Replace the restricted visibility allow-list.
   */
  reference_allowed_pack_refs?: Array<string> | null;
  reference_visibility?: null | ActionReferenceVisibility;
};
