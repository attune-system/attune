/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DeliverySegment } from "./DeliverySegment";
import type { ReserveCustody } from "./ReserveCustody";
export type PrepareCustody = {
  authenticated_manifest?: string | null;
  reservation: ReserveCustody;
  segments: Array<DeliverySegment>;
  sha256: string;
  total_bytes: number;
  truncated: boolean;
};
