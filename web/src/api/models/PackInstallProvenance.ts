/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ChecksumSubject } from "./ChecksumSubject";
export type PackInstallProvenance = {
  /**
   * Concrete installed artifact type (git, archive, or a local source type).
   */
  artifact_type: string;
  /**
   * Selected artifact URL or local path.
   */
  artifact_url?: string | null;
  /**
   * Canonical checksum in `algorithm:hash` form. For archives this covers
   * the downloaded artifact bytes; for Git and local sources it covers the
   * installed directory content.
   */
  checksum?: string | null;
  checksum_subject?: null | ChecksumSubject;
  /**
   * Whether the checksum was verified against its documented subject.
   */
  checksum_verified: boolean;
  /**
   * Whether installation fell back from the preferred registry artifact.
   */
  fallback_occurred: boolean;
  /**
   * Selected Git branch, tag, or commit, when applicable.
   */
  git_ref?: string | null;
  /**
   * API-managed registry row selected for resolution, when known.
   */
  registry_id?: number | null;
  /**
   * Registry index URL that resolved the pack, when applicable.
   */
  registry_url?: string | null;
  /**
   * Canonical resolved registry identity in `pack@version` form.
   */
  resolved_pack?: string | null;
};
