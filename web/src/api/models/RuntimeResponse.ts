/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Full runtime response.
 */
export type RuntimeResponse = {
  created: string;
  description?: string | null;
  distributions: Record<string, any>;
  execution_config: Record<string, any>;
  id: number;
  installation?: null | Value;
  name: string;
  pack?: number | null;
  pack_ref?: string | null;
  ref: string;
  retired_at?: string | null;
  updated: string;
};
