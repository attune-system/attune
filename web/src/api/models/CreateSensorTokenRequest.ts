/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request body for creating sensor tokens
 */
export type CreateSensorTokenRequest = {
  /**
   * Registered pack reference. Internal worker callers must provide it;
   * public callers may omit it and let the API resolve it.
   */
  pack_ref?: string | null;
  /**
   * Explicit sensor cache permission-set refs. `standard` grants read-only
   * access to the registered sensor and pack cache scopes.
   */
  permission_set_refs?: Array<string>;
  /**
   * Sensor reference (e.g., "core.timer")
   */
  sensor_ref: string;
  /**
   * List of trigger types this sensor can create events for
   */
  trigger_types: Array<string>;
  /**
   * Optional TTL in seconds (default: 86400 = 24 hours, max: 259200 = 72 hours)
   */
  ttl_seconds?: number | null;
};
