/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Sha256Digest } from "./Sha256Digest";
/**
 * Identity only. The secret lease credential travels in LAUNCH_LEASE_HEADER.
 * Engine resource IDs are evidence and never accepted as caller selectors.
 */
export type LaunchHandle = {
  admitted_digest: Sha256Digest;
  engine_id: number;
  engine_name_nonce: string;
  launch_id: number;
  lease_deadline: string;
  lease_generation: number;
  lease_id: string;
};
