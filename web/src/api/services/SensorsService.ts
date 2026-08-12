/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_SensorResponse } from "../models/ApiResponse_SensorResponse";
import type { CreateSensorRequest } from "../models/CreateSensorRequest";
import type { PaginatedResponse_SensorSummary } from "../models/PaginatedResponse_SensorSummary";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateSensorRequest } from "../models/UpdateSensorRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class SensorsService {
  /**
   * List sensors by pack reference
   * @returns PaginatedResponse_SensorSummary List of sensors in pack
   * @throws ApiError
   */
  public static listSensorsByPack({
    packRef,
    page,
    pageSize,
    q,
  }: {
    /**
     * Pack reference
     */
    packRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Keyword query. Whitespace-separated tokens are AND-matched against
     * the catalog item's discovery fields.
     */
    q?: string | null;
  }): CancelablePromise<PaginatedResponse_SensorSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/sensors",
      path: {
        pack_ref: packRef,
      },
      query: {
        page: page,
        page_size: pageSize,
        q: q,
      },
      errors: {
        404: `Pack not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * List all sensors with pagination
   * @returns PaginatedResponse_SensorSummary List of sensors
   * @throws ApiError
   */
  public static listSensors({
    page,
    pageSize,
    q,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Keyword query. Tokens are AND-matched across ref, label, description,
     * and pack_ref.
     */
    q?: string | null;
  }): CancelablePromise<PaginatedResponse_SensorSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/sensors",
      query: {
        page: page,
        page_size: pageSize,
        q: q,
      },
      errors: {
        500: `Internal server error`,
      },
    });
  }
  /**
   * Create a new sensor
   * @returns ApiResponse_SensorResponse Sensor created successfully
   * @throws ApiError
   */
  public static createSensor({
    requestBody,
  }: {
    requestBody: CreateSensorRequest;
  }): CancelablePromise<ApiResponse_SensorResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/sensors",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        404: `Pack, runtime, or trigger not found`,
        409: `Sensor with same ref already exists`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * List enabled sensors
   * @returns PaginatedResponse_SensorSummary List of enabled sensors
   * @throws ApiError
   */
  public static listEnabledSensors({
    page,
    pageSize,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_SensorSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/sensors/enabled",
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        500: `Internal server error`,
      },
    });
  }
  /**
   * Get a single sensor by reference
   * @returns ApiResponse_SensorResponse Sensor details
   * @throws ApiError
   */
  public static getSensor({
    ref,
  }: {
    /**
     * Sensor reference
     */
    ref: string;
  }): CancelablePromise<ApiResponse_SensorResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/sensors/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Sensor not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Update an existing sensor
   * @returns ApiResponse_SensorResponse Sensor updated successfully
   * @throws ApiError
   */
  public static updateSensor({
    ref,
    requestBody,
  }: {
    /**
     * Sensor reference
     */
    ref: string;
    requestBody: UpdateSensorRequest;
  }): CancelablePromise<ApiResponse_SensorResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/sensors/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        404: `Sensor not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Delete a sensor
   * @returns SuccessResponse Sensor deleted successfully
   * @throws ApiError
   */
  public static deleteSensor({
    ref,
  }: {
    /**
     * Sensor reference
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/sensors/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Sensor not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Disable a sensor
   * @returns ApiResponse_SensorResponse Sensor disabled successfully
   * @throws ApiError
   */
  public static disableSensor({
    ref,
  }: {
    /**
     * Sensor reference
     */
    ref: string;
  }): CancelablePromise<ApiResponse_SensorResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/sensors/{ref}/disable",
      path: {
        ref: ref,
      },
      errors: {
        404: `Sensor not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Enable a sensor
   * @returns ApiResponse_SensorResponse Sensor enabled successfully
   * @throws ApiError
   */
  public static enableSensor({
    ref,
  }: {
    /**
     * Sensor reference
     */
    ref: string;
  }): CancelablePromise<ApiResponse_SensorResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/sensors/{ref}/enable",
      path: {
        ref: ref,
      },
      errors: {
        404: `Sensor not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * List available log streams for a sensor.
   * @returns any Sensor log summary
   * @throws ApiError
   */
  public static listSensorLogs({
    sensorRef,
  }: {
    /**
     * Sensor reference (e.g., core.timer)
     */
    sensorRef: string;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/sensors/{sensor_ref}/logs",
      path: {
        sensor_ref: sensorRef,
      },
      errors: {
        401: `Unauthorized`,
      },
    });
  }
  /**
   * Download a specific sensor log stream.
   * Resolves the sensor ref + stream to a log file on disk and serves
   * the content as plain text.
   * @returns any Log file content
   * @throws ApiError
   */
  public static getSensorLog({
    sensorRef,
    stream,
  }: {
    /**
     * Sensor reference (e.g., core.timer)
     */
    sensorRef: string;
    /**
     * Log stream: stdout or stderr
     */
    stream: string;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/sensors/{sensor_ref}/logs/{stream}",
      path: {
        sensor_ref: sensorRef,
        stream: stream,
      },
      errors: {
        401: `Unauthorized`,
        404: `Sensor log not found`,
      },
    });
  }
  /**
   * List sensors by trigger reference
   * @returns PaginatedResponse_SensorSummary List of sensors for trigger
   * @throws ApiError
   */
  public static listSensorsByTrigger({
    triggerRef,
    page,
    pageSize,
  }: {
    /**
     * Trigger reference
     */
    triggerRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_SensorSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/triggers/{trigger_ref}/sensors",
      path: {
        trigger_ref: triggerRef,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
}
