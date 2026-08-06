/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class CrateRoutesSensorLogsService {
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
}
