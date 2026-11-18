/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchInputKind } from "./LaunchInputKind";
import type { Sha256Digest } from "./Sha256Digest";
/**
 * The body is fetched through the exact launch/pin endpoint, not a supplied
 * URL or tar. Body format is canonical ZIP (application/zip), with no fallback
 * reader. Size/hash and expanded quotas are verified before publication.
 * Runner input contains public immutable helper bytes, never credentials.
 */
export type PinnedLaunchInput = {
  kind: LaunchInputKind;
  max_expanded_bytes: number;
  max_files: number;
  object_id: number;
  object_version: string;
  pin_id: number;
  sha256: Sha256Digest;
  size_bytes: number;
};
