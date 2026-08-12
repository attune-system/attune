/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for downloading packs
 */
export type DownloadPacksRequest = {
  /**
   * List of pack sources (git URLs, HTTP URLs, or registry refs)
   */
  packs: Array<string>;
  /**
   * Git reference (branch, tag, or commit) for git sources
   */
  ref_spec?: string | null;
};
