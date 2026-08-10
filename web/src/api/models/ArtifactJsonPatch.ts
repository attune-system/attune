/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
export type ArtifactJsonPatch =
  | {
      op: ArtifactJsonPatch.op;
      value: Value;
    }
  | {
      op: ArtifactJsonPatch.op;
    };
export namespace ArtifactJsonPatch {
  export enum op {
    SET = "set",
  }
}
