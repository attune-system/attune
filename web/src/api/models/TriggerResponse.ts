/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
/**
 * Response DTO for trigger information
 */
export type TriggerResponse = {
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
   * Whether this is an ad-hoc trigger (not from pack installation)
   */
  is_adhoc: boolean;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Output schema
   */
  out_schema: any | null;
  /**
   * Pack ID (optional)
   */
  pack?: number | null;
  /**
   * Pack reference (optional)
   */
  pack_ref?: string | null;
  /**
   * Parameter schema (StackStorm-style with inline required/secret)
   */
  param_schema: any | null;
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
   * Sensor ID (optional — webhook triggers have no sensor)
   */
  sensor?: number | null;
  /**
   * Sensor reference (optional)
   */
  sensor_ref?: string | null;
  /**
   * Last update timestamp
   */
  updated: string;
  /**
   * Whether webhooks are enabled for this trigger
   */
  webhook_enabled: boolean;
  /**
   * Webhook key (only present if webhooks are enabled)
   */
  webhook_key?: string | null;
};
