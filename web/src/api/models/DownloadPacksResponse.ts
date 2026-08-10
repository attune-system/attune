/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DownloadedPack } from "./DownloadedPack";
import type { FailedPack } from "./FailedPack";
/**
 * Response DTO for download packs operation
 */
export type DownloadPacksResponse = {
  /**
   * Successfully downloaded packs
   */
  downloaded_packs: Array<DownloadedPack>;
  /**
   * Failed pack downloads
   */
  failed_packs: Array<FailedPack>;
  /**
   * Number of failed downloads
   */
  failure_count: number;
  /**
   * Number of successful downloads
   */
  success_count: number;
  /**
   * Total number of packs requested
   */
  total_count: number;
};
