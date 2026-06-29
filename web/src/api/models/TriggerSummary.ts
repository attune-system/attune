/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
/**
 * Simplified trigger response (for list endpoints)
 */
export type TriggerSummary = {
  /**
   * Creation timestamp
   */
  created: string;
  /**
   * Trigger description
   */
  description?: string | null;
  /**
   * Whether the trigger is enabled
   */
  enabled: boolean;
  /**
   * Trigger ID
   */
  id: number;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Pack reference (optional)
   */
  pack_ref?: string | null;
  /**
   * Pack refs allowed to subscribe to this trigger when visibility is restricted.
   */
  reference_allowed_pack_refs?: Array<string>;
  /**
   * Pack-level visibility for rule subscriptions.
   */
  reference_visibility: ActionReferenceVisibility;
  /**
   * Unique reference identifier
   */
  ref: string;
  /**
   * Last update timestamp
   */
  updated: string;
  /**
   * Whether webhooks are enabled for this trigger
   */
  webhook_enabled: boolean;
};
