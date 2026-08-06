/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class InternalService {
  /**
   * Download file content at the given path.
   * @returns any File content
   * @throws ApiError
   */
  public static downloadFile({
    filePath,
  }: {
    /**
     * Relative artifact file path
     */
    filePath: string;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/internal/files/{file_path}",
      path: {
        file_path: filePath,
      },
      errors: {
        400: `Invalid file path`,
        401: `Unauthorized`,
        404: `File not found`,
      },
    });
  }
  /**
   * Upload or overwrite a file at the given path.
   * The request body is the raw file content.
   * Content-Type header is stored alongside the file if needed.
   * @returns any File uploaded
   * @throws ApiError
   */
  public static uploadFile({
    filePath,
    requestBody,
  }: {
    /**
     * Relative artifact file path
     */
    filePath: string;
    requestBody: string;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/internal/files/{file_path}",
      path: {
        file_path: filePath,
      },
      body: requestBody,
      mediaType: "application/octet-stream",
      errors: {
        400: `Invalid file path`,
        401: `Unauthorized`,
        413: `Payload too large`,
      },
    });
  }
  /**
   * Wrapper to avoid conflict with the `delete` import from axum::routing
   * @returns void
   * @throws ApiError
   */
  public static deleteFileHandler({
    filePath,
  }: {
    /**
     * Relative artifact file path
     */
    filePath: string;
  }): CancelablePromise<void> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/internal/files/{file_path}",
      path: {
        file_path: filePath,
      },
      errors: {
        400: `Invalid file path`,
        401: `Unauthorized`,
        404: `File not found`,
      },
    });
  }
  /**
   * Check file existence and return size via HEAD request.
   * @returns any File exists; size is returned in Content-Length
   * @throws ApiError
   */
  public static checkFile({
    filePath,
  }: {
    /**
     * Relative artifact file path
     */
    filePath: string;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "HEAD",
      url: "/api/v1/internal/files/{file_path}",
      path: {
        file_path: filePath,
      },
      errors: {
        400: `Invalid file path`,
        401: `Unauthorized`,
        404: `File not found`,
      },
    });
  }
  /**
   * Append content to an existing file (or create it).
   * Used for streaming log writes — workers send periodic chunks.
   * @returns void
   * @throws ApiError
   */
  public static appendToFile({
    filePath,
    requestBody,
  }: {
    /**
     * Relative artifact file path
     */
    filePath: string;
    requestBody: string;
  }): CancelablePromise<void> {
    return __request(OpenAPI, {
      method: "PATCH",
      url: "/api/v1/internal/files/{file_path}",
      path: {
        file_path: filePath,
      },
      body: requestBody,
      mediaType: "application/octet-stream",
      errors: {
        400: `Invalid file path`,
        401: `Unauthorized`,
        413: `Payload too large`,
      },
    });
  }
  /**
   * Stream a pack directory as a `.tar.gz` archive.
   * Used by remote workers/sensors to download pack contents when they
   * don't share a mounted volume with the API.
   * @returns any Pack archive
   * @throws ApiError
   */
  public static downloadPackArchive({
    packRef,
  }: {
    /**
     * Pack reference identifier
     */
    packRef: string;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/internal/packs/{pack_ref}/archive",
      path: {
        pack_ref: packRef,
      },
      errors: {
        400: `Invalid pack reference`,
        401: `Unauthorized`,
        404: `Pack not found`,
      },
    });
  }
}
