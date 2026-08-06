/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardSourceContractResponse } from "./DashboardSourceContractResponse";
/**
 * Standard API response wrapper
 */
export type ApiResponse_DashboardSourceCatalogResponse = {
  data: {
    contracts: Array<DashboardSourceContractResponse>;
    source: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
