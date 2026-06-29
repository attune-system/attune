/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
export type IdentitySummary = {
  attributes: Value;
  display_name?: string | null;
  frozen: boolean;
  id: number;
  login: string;
  roles: Array<string>;
};
