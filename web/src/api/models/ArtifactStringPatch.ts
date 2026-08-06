/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type ArtifactStringPatch =
  | {
      op: ArtifactStringPatch.op;
      value: string;
    }
  | {
      op: ArtifactStringPatch.op;
    };
export namespace ArtifactStringPatch {
  export enum op {
    SET = "set",
  }
}
