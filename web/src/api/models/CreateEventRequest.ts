/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request body for creating an event
 */
export type CreateEventRequest = {
  /**
   * Event configuration
   */
  config?: any | null;
  /**
   * Event payload data
   */
  payload?: any | null;
  /**
   * Optional source trace tag for this event.
   * When omitted for execution-token callers, inherits from the parent execution.
   */
  trace_tag?: string | null;
  /**
   * Trigger instance ID (for correlation, often rule_id)
   */
  trigger_instance_id?: string | null;
  /**
   * Trigger reference (e.g., "core.timer", "core.webhook")
   * Also accepts "trigger_type" for compatibility with the sensor interface spec.
   */
  trigger_ref: string;
};
