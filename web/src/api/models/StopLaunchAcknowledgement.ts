/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EngineResourceEvidence } from "./EngineResourceEvidence";
import type { LaunchHandle } from "./LaunchHandle";
import type { ProviderCreateSettlement } from "./ProviderCreateSettlement";
/**
 * DefinitiveAbsence requires settled provider ownership, not a timeout/404.
 * Unknown creates remain StopRequired until owned resource settlement is proved.
 * A created resource requires Wait and successful Remove before acknowledgement.
 * Mere exit/stop is insufficient: an extant container could regain its RW stage.
 */
export type StopLaunchAcknowledgement = {
  create: ProviderCreateSettlement;
  handle: LaunchHandle;
  removed_resource?: null | EngineResourceEvidence;
};
