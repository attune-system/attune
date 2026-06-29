/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TaintEffect } from "./TaintEffect";
import type { TolerationOperator } from "./TolerationOperator";
export type WorkerToleration = {
  effect?: null | TaintEffect;
  key: string;
  operator?: TolerationOperator;
  value?: string | null;
};
