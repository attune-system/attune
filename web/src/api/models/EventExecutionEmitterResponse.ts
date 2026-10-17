/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
/**
 * Non-secret historical execution attribution. Requester and queue-item
 * attribution stay behind their own inspection permissions.
 */
export type EventExecutionEmitterResponse = {
  action_ref: string;
  admission_id: i64;
  attempt: string;
  execution_id: i64;
  pack_id: i64;
  pack_ref: string;
  policy_revision_id: i64;
  release_digest: string;
  release_id: i64;
  sponsor_identity_id: i64;
};
