/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TriggerResponse } from "./TriggerResponse";
/**
 * Standard API response wrapper
 */
export type ApiResponse_WebhookManagementResponse = {
  /**
   * Credential-bearing response, returned only by explicit management operations.
   */
  data: TriggerResponse & {
    webhook_key?: string | null;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
