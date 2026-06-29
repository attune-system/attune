/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { TriggerStringPatch } from "./TriggerStringPatch";
/**
 * Request DTO for updating a trigger
 */
export type UpdateTriggerRequest = {
  description?: null | TriggerStringPatch;
  /**
   * Whether the trigger is enabled
   */
  enabled?: boolean | null;
  /**
   * Human-readable label
   */
  label?: string | null;
  /**
   * Output schema
   */
  out_schema: any | null;
  /**
   * Parameter schema (StackStorm-style with inline required/secret)
   */
  param_schema: any | null;
  /**
   * Replace the restricted visibility allow-list.
   */
  reference_allowed_pack_refs?: any[] | null;
  /**
   * Pack-level visibility for rule subscriptions.
   */
  reference_visibility?: null | ActionReferenceVisibility;
};
