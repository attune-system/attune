/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for downloading packs
 */
export type DownloadPacksRequest = {
  /**
   * Destination directory for downloaded packs
   */
  destination_dir: string;
  /**
   * List of pack sources (git URLs, HTTP URLs, or registry refs)
   */
  packs: Array<string>;
  /**
   * Git reference (branch, tag, or commit) for git sources
   */
  ref_spec?: string | null;
  /**
   * Pack registry URL for resolving references
   */
  registry_url?: string | null;
  /**
   * Download timeout in seconds
   */
  timeout?: number;
  /**
   * Verify SSL certificates
   */
  verify_ssl?: boolean;
};
