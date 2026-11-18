/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Sha256Digest } from "./Sha256Digest";
/**
 * The complete request accepted by the engine-facing launch transport.
 * Authorization is carried separately; an admitted digest is not a credential.
 */
export type LaunchRequest = {
  admitted_digest: Sha256Digest;
  launch_id: number;
};
