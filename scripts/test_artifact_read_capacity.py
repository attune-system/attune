import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import artifact_read_capacity as capacity


class ArtifactCapacityTests(unittest.TestCase):
    def sample(self):
        return dict(volume_capacity_bytes=10000, unmodeled_used_bytes=1000,
            sample_seconds=10, reserve_fraction=.2, storage_safety_factor=1.25,
            cohorts=[dict(name='versions', inserted_rows=10, sampled_rows=10, sampled_total_bytes=100,
                          retention_seconds=100, backlog_seconds=100)])

    def test_forecast_distinguishes_retention_backlog_and_supplied_reserve(self):
        result = capacity.forecast(self.sample())
        self.assertEqual(result['modeled_retention_used_bytes'],2250)
        self.assertEqual(result['modeled_retention_and_backlog_used_bytes'],3500)
        self.assertEqual(result['reserve_bytes'],2000)
        self.assertEqual(result['growth_headroom_after_reserve_bytes'],4500)
        self.assertEqual(result['required_capacity_at_supplied_reserve_bytes'],4375)
        self.assertTrue(result['fits_with_backlog'])

    def test_backlog_can_cross_the_alert_threshold_even_when_retention_fits(self):
        result = capacity.forecast({**self.sample(),'volume_capacity_bytes':4000})
        self.assertTrue(result['fits_without_backlog'])
        self.assertFalse(result['fits_with_backlog'])
        self.assertEqual(result['growth_headroom_after_reserve_bytes'],-300)

    def test_invalid_or_ambiguous_observations_are_rejected(self):
        for change in ({'sample_seconds':0},{'volume_capacity_bytes':True},{'reserve_fraction':1},
                       {'reserve_fraction':float('nan')},{'storage_safety_factor':.9},{'cohorts':[]}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                capacity.forecast({**self.sample(),**change})
        for field in ('sampled_rows','sampled_total_bytes','retention_seconds'):
            sample=self.sample();sample['cohorts'][0][field]=0
            with self.assertRaises(ValueError):capacity.forecast(sample)
        sample=self.sample();sample['cohorts']*=2
        with self.assertRaises(ValueError):capacity.forecast(sample)

    def test_rounding_is_exact_at_bigint_scale(self):
        sample=self.sample();sample.update(volume_capacity_bytes=2**63-1,reserve_fraction=0,storage_safety_factor=1)
        sample['cohorts'][0].update(inserted_rows=10,sampled_rows=3,sampled_total_bytes=2**53+1,
                                  retention_seconds=1,backlog_seconds=0)
        result=capacity.forecast(sample)
        self.assertEqual(result['cohorts'][0]['retention_bytes'],(2**53+1+2)//3)

    def test_cli_supports_stdin_never_overwrites_and_preserves_negative_results(self):
        for size, expected in [(10000,0),(4000,1)]:
            with tempfile.TemporaryDirectory() as directory:
                path=Path(directory)/'report.json'
                with patch('sys.stdin',io.StringIO(json.dumps({**self.sample(),'volume_capacity_bytes':size}))):
                    self.assertEqual(capacity.main(['--input','-','--output',str(path)]),expected)
                content=path.read_text()
                with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                    capacity.main(['--input','-','--output',str(path)])
                self.assertEqual(error.exception.code,2)
                self.assertEqual(path.read_text(),content)


if __name__ == '__main__':
    unittest.main()
