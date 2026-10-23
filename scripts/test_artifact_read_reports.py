import unittest

from artifact_read_reports import compare_latency, compare_paired_reference, percentile, query_counts, summarize_explain, summarize_samples


class ArtifactReadReportTests(unittest.TestCase):
    def paired(self):
        return dict(comparison_kind='controlled-serial-reference-not-historical-revision',
                    identical_response_bodies=True, query_reduction_target=.90, p95_reduction_target=.70,
                    fixture='two-logs-100-executions', permissions='scoped-reader', concurrency=1,
                    cache_state='warm', completed_requests=20,
                    baseline=[dict(status=200,handler_seconds=1,queries=dict(statements=807)) for _ in range(20)],
                    treatment=[dict(status=200,handler_seconds=.1,queries=dict(statements=10)) for _ in range(20)])

    def test_controlled_reference_acceptance_does_not_claim_a_historical_speedup(self):
        result = compare_paired_reference(self.paired(), True)
        self.assertTrue(result['passed'])
        self.assertTrue(result['reference_exceeds_20_statement_ceiling'])
        self.assertFalse(result['historical_or_deployed_speedup_verified'])

    def test_paired_gate_fails_for_source_changes_target_failures_and_incomplete_work(self):
        sample = self.paired()
        self.assertFalse(compare_paired_reference(sample, False)['passed'])
        for change in ({'identical_response_bodies':False}, {'completed_requests':19}, {'query_reduction_target':.5},
                       {'baseline':sample['baseline'][:1]}, {'comparison_kind':'historical'}):
            self.assertFalse(compare_paired_reference({**sample,**change},True)['passed'])
        for status,seconds,statements in [(500,.1,10),(200,.9,10),(200,.1,21),(200,.1,200)]:
            report = self.paired()
            report['treatment'] = [dict(status=status,handler_seconds=seconds,queries=dict(statements=statements)) for _ in range(20)]
            self.assertFalse(compare_paired_reference(report,True)['passed'])

    def test_percentile_uses_nearest_rank_and_rejects_invalid_samples(self):
        self.assertEqual(percentile(list(range(1, 101)), .95), 95)
        for values in ([], [-1], [float('nan')], [float('inf')]):
            with self.assertRaises(ValueError):
                percentile(values, .95)

    def test_paired_query_gate_excludes_only_reported_infrastructure(self):
        report = self.paired()
        for sample in report['treatment']:
            sample['queries'] = dict(statements=31, statement_categories={
                'driver_type_discovery': 4, 'driver_enum_discovery': 4,
                'driver_type_name_discovery': 1,
                'pool_session_setup': 2, 'permission_set': 2,
            })
        result = compare_paired_reference(report, True)
        self.assertTrue(result['passed'])
        self.assertEqual(result['treatment_statements'], 620)
        self.assertEqual(result['treatment_application_statements'], 400)
        self.assertAlmostEqual(result['query_reduction_fraction'], 1 - 400 / (807 * 20))
        self.assertAlmostEqual(result['total_statement_reduction_fraction'], 1 - 620 / (807 * 20))
        for sample in report['treatment']:
            sample['queries']['statements'] = 32
        self.assertFalse(compare_paired_reference(report, True)['passed'])

    def test_infrastructure_cannot_make_the_reference_red(self):
        report = self.paired()
        for sample in report['baseline']:
            sample['queries'] = dict(statements=1000, statement_categories={'pool_session_setup': 990})
        self.assertFalse(compare_paired_reference(report, True)['passed'])

    def test_invalid_query_accounting_fails_closed(self):
        for queries in [dict(statements=-1), dict(statements=True),
                        dict(statements=2, statement_categories={'driver_enum_discovery': 3}),
                        dict(statements=2, statement_categories={'pool_session_setup': -1})]:
            with self.subTest(queries=queries), self.assertRaises(ValueError):
                query_counts(queries)
        self.assertEqual(query_counts(dict(statements=21))['application'], 21)

    def test_summary_reports_raw_application_and_infrastructure_costs_separately(self):
        sample = dict(status=200, handler_seconds=1, completion_seconds=2, response_bytes=3,
                      queries=dict(pool_acquire_seconds=.1, pool_acquisitions=2, statements=15,
                                   returned_rows=5, statement_categories={
                                       'driver_type_discovery': 2, 'driver_enum_discovery': 2,
                                       'driver_type_name_discovery': 1, 'pool_session_setup': 1}))
        summary = summarize_samples([sample, sample])
        self.assertEqual(summary['statements'], 30)
        self.assertEqual(summary['application_statements'], 18)
        self.assertEqual(summary['driver_metadata_statements'], 10)
        self.assertEqual(summary['pool_setup_statements'], 2)

    def test_buffers_are_not_summed_across_inclusive_nodes(self):
        plan = [{"Planning Time": 1, "Execution Time": 2, "Plan": {
            "Node Type": "Append", "Actual Rows": 3, "Actual Loops": 1, "Shared Hit Blocks": 10,
            "Plans": [
                {"Node Type": "Index Scan", "Relation Name": "event_p20200102", "Actual Loops": 1, "Shared Hit Blocks": 7},
                {"Node Type": "Seq Scan", "Relation Name": "event_default", "Actual Loops": 1, "Shared Hit Blocks": 3},
                {"Node Type": "Index Scan", "Relation Name": "event_p20200103", "Actual Loops": 0},
            ],
        }}]
        report = summarize_explain(plan)
        self.assertEqual(report['root_buffers_including_descendants']['Shared Hit Blocks'], 10)
        self.assertEqual(report['event_relations_executed'], ['event_default', 'event_p20200102'])
        self.assertEqual(report['returned_rows'], 3)

    def sample(self):
        return dict(fixture='two-logs-100-executions', permissions='scoped-reader', cache_state='warm',
                    concurrency=1, completed_requests=20, server_settings={'major': 18},
                    source_unchanged=True, seconds=[1] * 20)

    def test_matched_complete_samples_compare_the_predeclared_p95_target(self):
        baseline = self.sample()
        treatment = {**baseline, 'seconds': [.2] * 20}
        result = compare_latency(baseline, treatment)
        self.assertTrue(result['verified'])
        self.assertTrue(result['target_met'])
        self.assertAlmostEqual(result['reduction_fraction'], .8)

    def test_different_data_or_settings_do_not_certify_latency(self):
        for key in ('fixture', 'permissions', 'cache_state', 'concurrency', 'completed_requests', 'server_settings'):
            sample = self.sample()
            result = compare_latency(sample, {**sample, key: 'different'})
            self.assertFalse(result['verified'])
            self.assertIn(key, result['fields'])

    def test_changed_sources_errors_cancellations_and_missing_samples_do_not_certify(self):
        for change in ({'source_unchanged': False}, {'errors': 1}, {'cancelled': 1}, {'seconds': [1]}):
            sample = self.sample()
            self.assertFalse(compare_latency(sample, {**sample, **change})['verified'])

    def test_request_summary_keeps_completed_error_samples_and_measurement_availability(self):
        sample = dict(status=200, handler_seconds=1, completion_seconds=2, response_bytes=3,
                      handler_poll_cpu_seconds=None,
                      queries=dict(execution_disclosure_poll_cpu_seconds=None, pool_acquire_seconds=.1,
                                   pool_acquisitions=2, statements=4, returned_rows=5))
        result = summarize_samples([sample, {**sample, 'status': 500, 'handler_seconds': 10}])
        self.assertEqual(result['completed_requests'], 2)
        self.assertEqual(result['errors'], 1)
        self.assertEqual(result['p95_handler_seconds'], 10)
        self.assertEqual(result['pool_acquisitions'], 4)
        self.assertFalse(result['cpu_counter_available'])
        self.assertFalse(result['disclosure_cpu_counter_available'])


if __name__ == '__main__':
    unittest.main()
