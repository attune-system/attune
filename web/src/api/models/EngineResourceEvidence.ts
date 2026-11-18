/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Sha256Digest } from "./Sha256Digest";
export type EngineResourceEvidence = {
  admitted_digest: Sha256Digest;
  engine_id: number;
  engine_name_nonce: string;
  /**
   * Digest of the validated public engine manifest, excluding env/argv/stdin.
   */
  public_manifest_digest: Sha256Digest;
  resource_id: string;
};
