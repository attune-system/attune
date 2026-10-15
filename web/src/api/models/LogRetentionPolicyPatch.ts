/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RetentionPolicyType } from "./RetentionPolicyType";
export type LogRetentionPolicyPatch =
  | {
      op: "set";
      value: RetentionPolicyType;
    }
  | {
      op: "clear";
    };
