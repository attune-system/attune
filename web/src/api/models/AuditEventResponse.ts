/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
/**
 * Full audit event with all fields.
 */
export type AuditEventResponse = {
  actor_identity?: null | i64;
  /**
   * Source IP of the request.
   */
  actor_ip?: string | null;
  /**
   * Snapshot of `identity.login` at time of the event.
   */
  actor_login?: string | null;
  /**
   * Token type (`access`, `execution`, `sensor`, `refresh`).
   */
  actor_token_type?: string | null;
  /**
   * User-Agent header from the request.
   */
  actor_user_agent?: string | null;
  /**
   * High-level category.
   */
  category: string;
  /**
   * Optional cascade chain (rule_id, enforcement_id, execution_id, …).
   */
  correlation_chain: any | null;
  /**
   * Event creation timestamp.
   */
  created: string;
  /**
   * Event-specific structured metadata. Secrets are redacted.
   */
  details: any | null;
  /**
   * Request duration in milliseconds.
   */
  duration_ms?: number | null;
  /**
   * Dotted event-type identifier (e.g., `auth.login.success`).
   */
  event_type: string;
  /**
   * HTTP method (NULL for non-API events).
   */
  http_method?: string | null;
  /**
   * HTTP path.
   */
  http_path?: string | null;
  /**
   * HTTP status code.
   */
  http_status?: number | null;
  id: i64;
  /**
   * Outcome (`success`, `failure`, or `denied`).
   */
  outcome: string;
  /**
   * Correlation request ID assigned by the API middleware.
   */
  request_id?: string | null;
  resource_id?: null | i64;
  /**
   * Resource reference snapshot (forensic).
   */
  resource_ref?: string | null;
  /**
   * Logical resource type (e.g., `pack`, `key`, `execution`).
   */
  resource_type?: string | null;
};
