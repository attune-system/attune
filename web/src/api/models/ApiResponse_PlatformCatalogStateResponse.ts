/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PlatformCatalogStatus } from "./PlatformCatalogStatus";
/**
 * Standard API response wrapper
 */
export type ApiResponse_PlatformCatalogStateResponse = {
  data: {
    compatibility_epoch: number;
    expected_compatibility_epoch: number;
    expected_revision: number;
    revision: number;
    status: PlatformCatalogStatus;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
