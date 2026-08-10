/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
/**
 * Request DTO for creating a new trigger
 */
export type CreateTriggerRequest = {
  /**
   * Trigger description
   */
  description?: string | null;
  /**
   * Whether the trigger is enabled
   */
  enabled?: boolean;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Output schema (flat format) defining event data structure with inline required/secret
   */
  out_schema?: any | null;
  /**
   * Optional pack reference this trigger belongs to
   */
  pack_ref?: string | null;
  /**
   * Parameter schema (StackStorm-style) defining trigger configuration with inline required/secret
   */
  param_schema?: any | null;
  /**
   * Unique reference identifier (e.g., "core.webhook", "system.timer")
   */
  ref: string;
  /**
   * Pack refs allowed to subscribe to this trigger when visibility is restricted.
   */
  reference_allowed_pack_refs?: Array<string>;
  reference_visibility?: null | ActionReferenceVisibility;
};
