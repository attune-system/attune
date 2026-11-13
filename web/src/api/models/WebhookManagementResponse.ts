/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TriggerResponse } from "./TriggerResponse";
/**
 * Credential-bearing response, returned only by explicit management operations.
 */
export type WebhookManagementResponse = TriggerResponse & {
  webhook_key?: string | null;
};
