/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PreferredWorkerSelectorTerm } from "./PreferredWorkerSelectorTerm";
import type { WorkerSelectorTerm } from "./WorkerSelectorTerm";
export type WorkerAffinity = {
  anti_affinity?: Array<WorkerSelectorTerm>;
  preferred?: Array<PreferredWorkerSelectorTerm>;
  required?: Array<WorkerSelectorTerm>;
};
