/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { OutputVisibility } from "./OutputVisibility";
import type { StdoutPolicy } from "./StdoutPolicy";
/**
 * Optional authoring choices. Resolve against the stored output contract once.
 */
export type OutputPolicy = {
  error?: null | OutputVisibility;
  stderr?: null | OutputVisibility;
  stdout?: null | StdoutPolicy;
};
