/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchRoot } from "./LaunchRoot";
/**
 * Private CP materialization of the encrypted context. Never accept this from
 * a runner request, hash it into a public digest, or include its values in logs.
 * Per-stream keys, engine credentials and reusable enrollment are not fields.
 */
export type LaunchInvocation = {
  argv: Array<string>;
  env: Record<string, string>;
  working_root: LaunchRoot;
};
