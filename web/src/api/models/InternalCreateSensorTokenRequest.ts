/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request body for internal sensor token creation/reissue.
 *
 * Worker/service tokens must provide `sensor_ref` and `trigger_types`.
 * Sensor-token refresh calls may omit those fields; the server will derive them
 * from authenticated identity state.
 */
export type InternalCreateSensorTokenRequest = {
  /**
   * Current sensor workload assignment generation (required for worker/service callers).
   */
  assignment_generation?: number | null;
  /**
   * Registered pack reference (required for worker/service callers).
   */
  pack_ref?: string | null;
  /**
   * Explicit cache permission-set refs (required, though it may be empty,
   * for worker/service callers).
   */
  permission_set_refs?: any[] | null;
  /**
   * Sensor reference (required for worker/service callers)
   */
  sensor_ref?: string | null;
  /**
   * List of trigger types this sensor can create events for (required for worker/service callers)
   */
  trigger_types?: any[] | null;
  /**
   * Optional TTL in seconds (default: 86400 = 24 hours, max: 259200 = 72 hours)
   */
  ttl_seconds?: number | null;
  /**
   * Worker process instance that owns the assignment (required for worker/service callers).
   */
  worker_instance?: string | null;
  /**
   * Assigned sensor workload ID (required for worker/service callers).
   */
  workload_id?: number | null;
};
