/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Response for pack workflow validation operation
 */
export type PackWorkflowValidationResponse = {
  /**
   * Number of workflows with errors
   */
  error_count: number;
  /**
   * Validation errors by workflow reference
   */
  errors: Record<string, Array<string>>;
  /**
   * Pack reference
   */
  pack_ref: string;
  /**
   * Number of workflows validated
   */
  validated_count: number;
};
