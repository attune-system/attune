/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_WebhookReceiverResponse = {
  /**
   * Response from webhook receiver endpoint
   */
  data: {
    /**
     * ID of the event created from this webhook
     */
    event_id: number;
    /**
     * Success message
     */
    message: string;
    /**
     * Timestamp when the webhook was received
     */
    received_at: string;
    /**
     * Reference of the trigger that received this webhook
     */
    trigger_ref: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
