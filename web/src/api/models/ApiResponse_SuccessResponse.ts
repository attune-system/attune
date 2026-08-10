/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_SuccessResponse = {
  /**
   * Success message response (for operations that don't return data)
   */
  data: {
    /**
     * Message describing the operation
     */
    message: string;
    /**
     * Success indicator
     */
    success: boolean;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
