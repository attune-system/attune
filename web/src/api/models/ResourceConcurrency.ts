/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PolicyMethod } from "./PolicyMethod";
import type { ResourceParticipant } from "./ResourceParticipant";
export type ResourceConcurrency = {
  limit: number;
  method: PolicyMethod;
  namespace: string;
  participants: Array<ResourceParticipant>;
};
