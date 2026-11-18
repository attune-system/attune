/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchHandle } from "./LaunchHandle";
import type { LaunchOutputKind } from "./LaunchOutputKind";
import type { Sha256Digest } from "./Sha256Digest";
export type ReserveLaunchOutput = {
  handle: LaunchHandle;
  kind: LaunchOutputKind;
  /**
   * StageArchive requires successful Exited + Stopped (settled create/removal),
   * never CleanupRequired or StopRequired. Its body is canonical ZIP.
   * For protected output this is the ciphertext body hash, not plaintext.
   */
  sha256: Sha256Digest;
  size_bytes: number;
};
