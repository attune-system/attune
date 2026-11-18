/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Raw stdin/stdout/stderr are distinct from encoded/published log artifacts.
 * These credentials stay in non-pack managers/custodians, not invocation envs.
 */
export enum LaunchIoChannel {
  STDIN = "stdin",
  STDOUT = "stdout",
  STDERR = "stderr",
}
