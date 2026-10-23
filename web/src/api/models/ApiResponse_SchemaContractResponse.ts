/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_SchemaContractResponse = {
  data: {
    /**
     * Self-contained rendering graph. Preserve authored references in editors.
     */
    contract: any;
    semantic_digest: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
