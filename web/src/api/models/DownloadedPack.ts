/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Information about a downloaded pack
 */
export type DownloadedPack = {
  /**
   * Directory checksum
   */
  checksum?: string | null;
  /**
   * Git commit hash (for git sources)
   */
  git_commit?: string | null;
  /**
   * Local path to downloaded pack
   */
  pack_path: string;
  /**
   * Pack reference from pack.yaml
   */
  pack_ref: string;
  /**
   * Pack version from pack.yaml
   */
  pack_version: string;
  /**
   * Original source
   */
  source: string;
  /**
   * Source type (git, http, registry)
   */
  source_type: string;
};
