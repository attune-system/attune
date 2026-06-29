/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Request body for webhook receiver endpoint
 */
export type WebhookReceiverRequest = {
  headers?: null | Value;
  /**
   * Webhook payload (arbitrary JSON)
   */
  payload: Value;
  /**
   * Optional source IP address
   */
  source_ip?: string | null;
  /**
   * Optional user agent
   */
  user_agent?: string | null;
};
