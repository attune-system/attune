/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for downloading packs
 */
export type DownloadPacksRequest = {
  /**
   * List of explicit Git or archive URLs. Registry refs must use /packs/install.
   */
  packs: Array<string>;
  /**
   * Git reference (branch, tag, or commit) for git sources
   */
  ref_spec?: string | null;
};
