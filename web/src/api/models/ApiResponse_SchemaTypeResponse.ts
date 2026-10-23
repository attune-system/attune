/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_SchemaTypeResponse = {
  data: {
    description?: string | null;
    id: number;
    label: string;
    managed_release?: number | null;
    pack_ref: string;
    ref: string;
    reference_allowed_pack_refs: Array<string>;
    /**
     * Pack-reference policy, not a restriction on authenticated inspection or
     * Key schema assignment.
     */
    reference_visibility: string;
    schema: any;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
