/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Namespace-level publication policy overrides. Unspecified fields keep their
 * existing (or default) values.
 */
export type CacheNamespacePolicyBody = {
  freshness_target_seconds?: number | null;
  max_generation_bytes?: number | null;
  max_records_per_generation?: number | null;
  max_retained_bytes?: number | null;
  /**
   * Number of published generations retained. At least two are required so
   * readers can complete traversal of the prior snapshot after promotion.
   */
  max_retained_generations?: number | null;
  max_staging_generations?: number | null;
};
