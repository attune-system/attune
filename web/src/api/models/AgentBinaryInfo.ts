/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AgentArchInfo } from "./AgentArchInfo";
/**
 * Agent binary metadata
 */
export type AgentBinaryInfo = {
  /**
   * Available architectures
   */
  architectures: Array<AgentArchInfo>;
  /**
   * Agent version (from build)
   */
  version: string;
};
