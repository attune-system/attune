/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TriggerResponse } from "../models/TriggerResponse";
import type { WebhookReceiverRequest } from "../models/WebhookReceiverRequest";
import type { WebhookReceiverResponse } from "../models/WebhookReceiverResponse";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class WebhooksService {
  /**
   * Disable webhooks for a trigger
   * @returns TriggerResponse Webhooks disabled
   * @throws ApiError
   */
  public static disableWebhook({
    ref,
  }: {
    /**
     * Trigger reference (pack.name)
     */
    ref: string;
  }): CancelablePromise<TriggerResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/triggers/{ref}/webhooks/disable",
      path: {
        ref: ref,
      },
      errors: {
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Enable webhooks for a trigger
   * @returns TriggerResponse Webhooks enabled
   * @throws ApiError
   */
  public static enableWebhook({
    ref,
  }: {
    /**
     * Trigger reference (pack.name)
     */
    ref: string;
  }): CancelablePromise<TriggerResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/triggers/{ref}/webhooks/enable",
      path: {
        ref: ref,
      },
      errors: {
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Regenerate webhook key for a trigger
   * @returns TriggerResponse Webhook key regenerated
   * @throws ApiError
   */
  public static regenerateWebhookKey({
    ref,
  }: {
    /**
     * Trigger reference (pack.name)
     */
    ref: string;
  }): CancelablePromise<TriggerResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/triggers/{ref}/webhooks/regenerate",
      path: {
        ref: ref,
      },
      errors: {
        400: `Webhooks not enabled for this trigger`,
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Webhook receiver endpoint - receives webhook events and creates events
   * @returns WebhookReceiverResponse Webhook received and event created
   * @throws ApiError
   */
  public static receiveWebhook({
    webhookKey,
    requestBody,
  }: {
    /**
     * Webhook key
     */
    webhookKey: string;
    requestBody: WebhookReceiverRequest;
  }): CancelablePromise<WebhookReceiverResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/webhooks/{webhook_key}",
      path: {
        webhook_key: webhookKey,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Invalid webhook key`,
        429: `Rate limit exceeded`,
        500: `Internal server error`,
      },
    });
  }
}
