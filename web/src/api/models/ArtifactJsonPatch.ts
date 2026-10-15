/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
export type ArtifactJsonPatch =
  | {
      op: "set";
      value: Value;
    }
  | {
      op: "clear";
    };
