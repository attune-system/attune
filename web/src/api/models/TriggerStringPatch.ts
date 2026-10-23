/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TriggerClearOperation } from "./TriggerClearOperation";
import type { TriggerSetOperation } from "./TriggerSetOperation";
export type TriggerStringPatch =
  | {
      op: TriggerSetOperation;
      value: string;
    }
  | {
      op: TriggerClearOperation;
    };
