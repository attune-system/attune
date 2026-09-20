/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ExternalActorAssertion } from "./ExternalActorAssertion";
/**
 * Provider-neutral one-shot response submitted by an integration adapter.
 */
export type ExternalInquiryRespondRequest = {
  external_actor: ExternalActorAssertion;
  response: Record<string, any>;
  response_handle: string;
};
