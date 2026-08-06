/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { NodeJsRequirements } from "./NodeJsRequirements";
import type { PythonRequirements } from "./PythonRequirements";
/**
 * Runtime requirements for a pack
 */
export type RuntimeRequirements = {
  nodejs?: null | NodeJsRequirements;
  /**
   * Pack reference
   */
  pack_ref: string;
  python?: null | PythonRequirements;
};
