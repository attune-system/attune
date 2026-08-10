/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { FailedPackRegistration } from "./FailedPackRegistration";
import type { RegisteredPack } from "./RegisteredPack";
import type { RegistrationSummary } from "./RegistrationSummary";
/**
 * Response DTO for register packs operation
 */
export type RegisterPacksResponse = {
  /**
   * Failed pack registrations
   */
  failed_packs: Array<FailedPackRegistration>;
  /**
   * Successfully registered packs
   */
  registered_packs: Array<RegisteredPack>;
  /**
   * Summary statistics
   */
  summary: RegistrationSummary;
};
