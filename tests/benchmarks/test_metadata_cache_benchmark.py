from pathlib import Path
import sys

BENCHMARKS_DIR = Path(__file__).resolve().parent
if str(BENCHMARKS_DIR) not in sys.path:
    sys.path.insert(0, str(BENCHMARKS_DIR))

import metadata_cache_benchmark as bench


def test_summarize_samples_reports_expected_percentiles():
    summary = bench.summarize_samples([10.0, 20.0, 30.0, 40.0, 50.0])
    assert summary["count"] == 5
    assert summary["min"] == 10.0
    assert summary["max"] == 50.0
    assert summary["p50"] == 30.0
    assert summary["p95"] == 48.0
    assert summary["p99"] == 49.6


def test_compare_metric_respects_lower_is_better():
    comparison = bench.compare_metric(90.0, 100.0, lower_is_better=True)
    assert comparison["winner"] == "cache-on"
    assert comparison["delta"] == -10.0
    assert comparison["delta_percent"] == -10.0


def test_unwrap_items_handles_paginated_data_object():
    payload = {"data": {"items": [{"id": 1}, {"id": 2}], "total": 2}}
    assert bench.unwrap_items(payload) == [{"id": 1}, {"id": 2}]


def test_distribute_child_counts_spreads_remainder():
    assert bench.BenchmarkRunner.distribute_child_counts(10, 3) == [4, 3, 3]
    assert bench.BenchmarkRunner.distribute_child_counts(2, 4) == [1, 1]


def test_build_settings_applies_profile_and_overrides():
    class Args:
        profile = "execution-throughput"
        base_url = None
        metadata_warmup_seconds = None
        metadata_duration_seconds = None
        metadata_concurrency = None
        metadata_seed_count = None
        execution_warmup_count = None
        execution_count = 900
        execution_concurrency = None
        execution_workflow_parent_count = None
        execution_measurement_rounds = None
        e2e_warmup_iterations = None
        e2e_pipelines = None
        e2e_iterations = None
        e2e_measurement_rounds = None
        poll_interval_seconds = None
        e2e_poll_interval_seconds = None
        execution_timeout_seconds = None
        sampler_interval_seconds = None
        hotpath_warmup_iterations = None
        hotpath_iterations = None
        hotpath_concurrency = None
        queue_poll_settle_seconds = None

    settings = bench.build_settings_from_args(Args())
    assert settings.profile == "execution-throughput"
    assert settings.execution_workflow_parent_count == 4
    assert settings.execution_count == 900
    assert settings.execution_measurement_rounds == 3
    assert settings.e2e_measurement_rounds == 3


def test_soak_profile_enables_steady_state_hotpath():
    class Args:
        profile = "soak"

    settings = bench.build_settings_from_args(Args())
    assert settings.steady_state_windows == 6
    assert settings.steady_state_iterations_per_window == 160
    assert settings.steady_state_concurrency == 16
    assert settings.execution_count >= 1000
    assert settings.execution_measurement_rounds == 6
    assert settings.e2e_measurement_rounds == 6


def test_select_median_throughput_round_ignores_extreme_outlier():
    rounds = [
        {"round_index": 0, "completed_per_second": 10.0, "schedule_latency_ms": {"p95": 900.0}},
        {"round_index": 1, "completed_per_second": 30.0, "schedule_latency_ms": {"p95": 300.0}},
        {"round_index": 2, "completed_per_second": 20.0, "schedule_latency_ms": {"p95": 500.0}},
    ]

    selected = bench.BenchmarkRunner.select_median_throughput_round(rounds)

    assert selected["round_index"] == 2


def test_select_median_latency_round_ignores_extreme_outlier():
    rounds = [
        {"round_index": 0, "latency_ms": {"p95": 100.0, "p99": 110.0}},
        {"round_index": 1, "latency_ms": {"p95": 300.0, "p99": 330.0}},
        {"round_index": 2, "latency_ms": {"p95": 200.0, "p99": 210.0}},
    ]

    selected = bench.BenchmarkRunner.select_median_latency_round(rounds)

    assert selected["round_index"] == 2


def test_select_median_latency_round_ignores_fully_failed_round():
    rounds = [
        {"round_index": 0, "latency_ms": {"p95": None, "p99": None}, "success_rate": 0.0},
        {"round_index": 1, "latency_ms": {"p95": 100.0, "p99": 110.0}, "success_rate": 1.0},
        {"round_index": 2, "latency_ms": {"p95": 200.0, "p99": 210.0}, "success_rate": 1.0},
    ]

    selected = bench.BenchmarkRunner.select_median_latency_round(rounds)

    assert selected["round_index"] == 2


def test_run_e2e_measurement_round_records_timeout_without_raising():
    class Settings:
        e2e_iterations = 2
        e2e_pipelines = 1
        e2e_warmup_iterations = 0
        e2e_poll_interval_seconds = 0.05

    runner = object.__new__(bench.BenchmarkRunner)
    runner.settings = Settings()
    runner.pipelines = [bench.E2EPipeline("marker", "webhook", "trigger")]
    calls = {"count": 0}

    def fake_fire(_pipeline, _seen):
        calls["count"] += 1
        if calls["count"] == 1:
            raise TimeoutError("No execution observed for pipeline marker")
        return {
            "status": "completed",
            "latency_ms": 10.0,
            "post_latency_ms": 1.0,
            "discovery_latency_ms": 2.0,
            "terminal_wait_latency_ms": 8.0,
            "event_to_execution_created_ms": 3.0,
            "execution_internal_ms": 7.0,
        }

    runner.fire_pipeline_once = fake_fire

    result = bench.BenchmarkRunner.run_e2e_measurement_round(runner, 0)

    assert result["requests_attempted"] == 2
    assert result["requests_completed"] == 1
    assert result["failure_count"] == 1
    assert result["status_counts"] == {"timeout": 1, "completed": 1}
    assert result["success_rate"] == 0.5
    assert result["latency_ms"]["count"] == 1
    assert result["failures"][0]["status"] == "timeout"


def test_valkey_delta_defaults_to_zero_for_missing_stats():
    assert bench.valkey_delta({}, "keyspace_hits") == 0
    assert bench.valkey_delta({"valkey": {"delta": {"keyspace_hits": 42}}}, "keyspace_hits") == 42


def test_metadata_cache_delta_defaults_to_zero_for_missing_stats():
    assert bench.metadata_cache_delta({}, "l1_json_hits") == 0
    assert bench.metadata_cache_delta({"metadata_cache": {"delta": {"l1_json_hits": 42}}}, "l1_json_hits") == 42


def test_collect_analytics_parses_response_json_payload():
    class FakeResponse:
        def raise_for_status(self):
            return None

        def json(self):
            return {"data": {"executions": {"completed": 7}}}

    class FakeClient:
        def get(self, path, params):
            assert path == "/api/v1/analytics/dashboard"
            assert "since" in params
            assert "until" in params
            return FakeResponse()

    runner = object.__new__(bench.BenchmarkRunner)
    runner.client = FakeClient()
    runner.suite_started_at = bench.utc_now()

    analytics = bench.BenchmarkRunner.collect_analytics(runner)

    assert analytics["payload"] == {"executions": {"completed": 7}}
    assert "error" not in analytics


def test_percentile_sample_warnings_find_small_samples():
    warnings = bench.percentile_sample_warnings(
        {
            "hot_path_cache": {
                "latency_ms": {"count": 40, "p95": 10.0, "p99": 20.0},
            },
            "metadata_api": {
                "latency_ms": {"count": 250, "p95": 10.0, "p99": 20.0},
            },
        }
    )
    assert warnings == [
        {
            "workload": "hot_path_cache",
            "metric": "latency_ms",
            "count": 40,
            "message": "hot_path_cache.latency_ms p95/p99 are based on only 40 samples",
        }
    ]


def test_profile_interpretation_notes_identify_smoke_profile():
    notes = bench.profile_interpretation_notes(
        {"settings": {"profile": "smoke", "execution_count": 60, "e2e_pipelines": 4, "e2e_iterations": 6}}
    )
    assert any("smoke profile" in note for note in notes)
    assert any("60 child executions" in note for note in notes)
    assert any("24 end-to-end samples" in note for note in notes)


def test_build_comparison_extracts_expected_metrics():
    cache_on = {
        "mode": "cache-on",
        "metadata_api": {"requests_per_second": 120.0, "latency_ms": {"p95": 12.0, "p99": 18.0}},
        "execution_throughput": {
            "completed_per_second": 14.0,
            "schedule_latency_ms": {"p95": 300.0},
            "end_to_end_latency_ms": {"p95": 800.0},
        },
        "hot_path_cache": {"requests_per_second": 75.0, "latency_ms": {"p95": 30.0, "p99": 45.0}},
        "automation_latency": {"latency_ms": {"p95": 950.0, "p99": 1100.0}, "success_rate": 1.0},
    }
    cache_off = {
        "mode": "cache-off",
        "metadata_api": {"requests_per_second": 100.0, "latency_ms": {"p95": 15.0, "p99": 24.0}},
        "execution_throughput": {
            "completed_per_second": 10.0,
            "schedule_latency_ms": {"p95": 420.0},
            "end_to_end_latency_ms": {"p95": 980.0},
        },
        "hot_path_cache": {"requests_per_second": 50.0, "latency_ms": {"p95": 50.0, "p99": 80.0}},
        "automation_latency": {"latency_ms": {"p95": 1200.0, "p99": 1400.0}, "success_rate": 1.0},
    }
    comparison = bench.build_comparison(cache_on, cache_off)
    assert comparison["metrics"]["metadata_api.requests_per_second"]["winner"] == "cache-on"
    assert comparison["metrics"]["metadata_api.latency_ms.p95"]["winner"] == "cache-on"
    assert comparison["metrics"]["execution_throughput.completed_per_second"]["winner"] == "cache-on"
    assert comparison["metrics"]["hot_path_cache.requests_per_second"]["winner"] == "cache-on"


def test_build_comparison_omits_steady_state_metrics_when_workload_absent():
    cache_on = {
        "metadata_api": {"requests_per_second": 1.0, "latency_ms": {"p95": 1.0, "p99": 1.0}},
        "execution_throughput": {
            "round_index": 1,
            "all_rounds": [
                {
                    "round_index": 0,
                    "completed_per_second": 0.5,
                    "schedule_latency_ms": {"p95": 4.0},
                    "end_to_end_latency_ms": {"p95": 5.0},
                    "internal_elapsed_seconds": 2.0,
                    "completed": 1,
                },
                {
                    "round_index": 1,
                    "completed_per_second": 1.0,
                    "schedule_latency_ms": {"p95": 1.0},
                    "end_to_end_latency_ms": {"p95": 1.0},
                    "internal_elapsed_seconds": 1.0,
                    "completed": 1,
                },
            ],
            "completed_per_second": 1.0,
            "schedule_latency_ms": {"p95": 1.0},
            "end_to_end_latency_ms": {"p95": 1.0},
        },
        "hot_path_cache": {"requests_per_second": 1.0, "latency_ms": {"p95": 1.0, "p99": 1.0}},
        "automation_latency": {"latency_ms": {"p95": 1.0, "p99": 1.0}, "success_rate": 1.0},
    }
    comparison = bench.build_comparison(cache_on, cache_on)
    assert not any(name.startswith("steady_state_hot_path.") for name in comparison["metrics"])


def test_render_comparison_includes_cache_engagement_table():
    cache_on = {
        "mode": "cache-on",
        "settings": {"profile": "smoke", "execution_count": 60, "e2e_pipelines": 4, "e2e_iterations": 6},
        "environment": {},
        "metadata_api": {
            "requests_per_second": 1.0,
            "latency_ms": {"p95": 1.0, "p99": 1.0},
            "valkey": {"delta": {"keyspace_hits": 5, "total_commands_processed": 10}},
            "metadata_cache": {"delta": {"l1_json_hits": 11, "l1_json_misses": 3, "l1_index_hits": 7, "l1_index_misses": 2, "l2_json_hits": 5, "l2_json_misses": 1, "l2_index_hits": 4, "l2_index_misses": 1, "local_only_fallbacks": 0, "writes": 2, "evictions": 1, "errors": 0}},
        },
        "execution_throughput": {
            "round_index": 0,
            "all_rounds": [
                {
                    "round_index": 0,
                    "completed_per_second": 1.0,
                    "schedule_latency_ms": {"p95": 1.0},
                    "end_to_end_latency_ms": {"p95": 1.0},
                    "internal_elapsed_seconds": 1.0,
                    "completed": 1,
                }
            ],
            "completed_per_second": 1.0,
            "schedule_latency_ms": {"p95": 1.0},
            "end_to_end_latency_ms": {"p95": 1.0},
            "valkey": {"delta": {"keyspace_hits": 6, "total_commands_processed": 12}},
            "metadata_cache": {"delta": {"l1_json_hits": 0, "l1_json_misses": 0, "l1_index_hits": 0, "l1_index_misses": 0, "l2_json_hits": 0, "l2_json_misses": 0, "l2_index_hits": 0, "l2_index_misses": 0, "local_only_fallbacks": 0, "writes": 0, "evictions": 0, "errors": 0}},
        },
        "hot_path_cache": {
            "requests_per_second": 1.0,
            "latency_ms": {"count": 40, "p95": 1.0, "p99": 1.0},
            "valkey": {"delta": {"keyspace_hits": 8, "total_commands_processed": 16}},
            "metadata_cache": {"delta": {"l1_json_hits": 2, "l1_json_misses": 1, "l1_index_hits": 3, "l1_index_misses": 1, "l2_json_hits": 1, "l2_json_misses": 1, "l2_index_hits": 1, "l2_index_misses": 0, "local_only_fallbacks": 0, "writes": 0, "evictions": 0, "errors": 0}},
        },
        "automation_latency": {
            "round_index": 1,
            "all_rounds": [
                {
                    "round_index": 0,
                    "latency_ms": {"p95": 2.0, "p99": 3.0},
                    "terminal_wait_latency_ms": {"p99": 2.0},
                    "execution_internal_ms": {"p99": 2.0},
                    "success_rate": 1.0,
                    "requests_completed": 1,
                },
                {
                    "round_index": 1,
                    "latency_ms": {"p95": 1.0, "p99": 1.0},
                    "terminal_wait_latency_ms": {"p99": 1.0},
                    "execution_internal_ms": {"p99": 1.0},
                    "success_rate": 1.0,
                    "requests_completed": 1,
                },
            ],
            "latency_ms": {"p95": 1.0, "p99": 1.0},
            "success_rate": 1.0,
            "valkey": {"delta": {"keyspace_hits": 7, "total_commands_processed": 14}},
            "metadata_cache": {"delta": {"l1_json_hits": 0, "l1_json_misses": 0, "l1_index_hits": 0, "l1_index_misses": 0, "l2_json_hits": 0, "l2_json_misses": 0, "l2_index_hits": 0, "l2_index_misses": 0, "local_only_fallbacks": 0, "writes": 0, "evictions": 0, "errors": 0}},
        },
        "steady_state_hot_path": {
            "requests_per_second": 1.0,
            "latency_ms": {"p95": 1.0, "p99": 1.0},
            "last_window_latency_ms": {"p95": 1.0},
            "p95_drift_percent": 0.0,
            "valkey": {"delta": {"keyspace_hits": 9, "total_commands_processed": 18}},
            "metadata_cache": {"delta": {"l1_json_hits": 1, "l1_json_misses": 0, "l1_index_hits": 1, "l1_index_misses": 0, "l2_json_hits": 1, "l2_json_misses": 0, "l2_index_hits": 1, "l2_index_misses": 0, "local_only_fallbacks": 0, "writes": 0, "evictions": 0, "errors": 0}},
            "windows": [
                {
                    "window_index": 0,
                    "requests_per_second": 10.0,
                    "latency_ms": {"mean": 5.0, "p95": 8.0, "p99": 9.0},
                    "metadata_cache": {"delta": {"l1_json_hits": 3, "l1_index_hits": 2, "l2_json_hits": 1, "l2_index_hits": 1}},
                    "valkey": {"delta": {"total_commands_processed": 4}},
                }
            ],
        },
    }
    cache_off = {
        "mode": "cache-off",
        "metadata_api": {
            "requests_per_second": 1.0,
            "latency_ms": {"p95": 1.0, "p99": 1.0},
            "valkey": {"delta": {"keyspace_hits": 0, "total_commands_processed": 2}},
            "metadata_cache": {"delta": {"l1_json_hits": 4, "l1_json_misses": 2, "l1_index_hits": 1, "l1_index_misses": 1, "l2_json_hits": 0, "l2_json_misses": 0, "l2_index_hits": 0, "l2_index_misses": 0, "local_only_fallbacks": 3, "writes": 1, "evictions": 1, "errors": 0}},
        },
        "execution_throughput": {
            "completed_per_second": 1.0,
            "schedule_latency_ms": {"p95": 1.0},
            "end_to_end_latency_ms": {"p95": 1.0},
            "valkey": {"delta": {"keyspace_hits": 0, "total_commands_processed": 2}},
            "metadata_cache": {"delta": {"l1_json_hits": 0, "l1_json_misses": 0, "l1_index_hits": 0, "l1_index_misses": 0, "l2_json_hits": 0, "l2_json_misses": 0, "l2_index_hits": 0, "l2_index_misses": 0, "local_only_fallbacks": 0, "writes": 0, "evictions": 0, "errors": 0}},
        },
        "hot_path_cache": {
            "requests_per_second": 1.0,
            "latency_ms": {"count": 40, "p95": 1.0, "p99": 1.0},
            "valkey": {"delta": {"keyspace_hits": 0, "total_commands_processed": 2}},
            "metadata_cache": {"delta": {"l1_json_hits": 0, "l1_json_misses": 0, "l1_index_hits": 0, "l1_index_misses": 0, "l2_json_hits": 0, "l2_json_misses": 0, "l2_index_hits": 0, "l2_index_misses": 0, "local_only_fallbacks": 0, "writes": 0, "evictions": 0, "errors": 0}},
        },
        "automation_latency": {
            "round_index": 0,
            "all_rounds": [
                {
                    "round_index": 0,
                    "latency_ms": {"p95": 1.0, "p99": 1.0},
                    "terminal_wait_latency_ms": {"p99": 1.0},
                    "execution_internal_ms": {"p99": 1.0},
                    "success_rate": 1.0,
                    "requests_completed": 1,
                }
            ],
            "latency_ms": {"p95": 1.0, "p99": 1.0},
            "success_rate": 1.0,
            "valkey": {"delta": {"keyspace_hits": 0, "total_commands_processed": 2}},
            "metadata_cache": {"delta": {"l1_json_hits": 0, "l1_json_misses": 0, "l1_index_hits": 0, "l1_index_misses": 0, "l2_json_hits": 0, "l2_json_misses": 0, "l2_index_hits": 0, "l2_index_misses": 0, "local_only_fallbacks": 0, "writes": 0, "evictions": 0, "errors": 0}},
        },
        "steady_state_hot_path": {
            "requests_per_second": 1.0,
            "latency_ms": {"p95": 1.0, "p99": 1.0},
            "last_window_latency_ms": {"p95": 1.0},
            "p95_drift_percent": 0.0,
            "valkey": {"delta": {"keyspace_hits": 0, "total_commands_processed": 2}},
            "metadata_cache": {"delta": {"l1_json_hits": 0, "l1_json_misses": 0, "l1_index_hits": 0, "l1_index_misses": 0, "l2_json_hits": 0, "l2_json_misses": 0, "l2_index_hits": 0, "l2_index_misses": 0, "local_only_fallbacks": 1, "writes": 0, "evictions": 0, "errors": 0}},
            "windows": [],
        },
    }
    comparison = bench.build_comparison(cache_on, cache_off)
    markdown = bench.render_comparison_markdown(cache_on, cache_off, comparison)
    assert "## Cache engagement" in markdown
    assert "## Execution throughput rounds" in markdown
    assert "## Automation latency rounds" in markdown
    assert "median `completed_per_second` round" in markdown
    assert "median latency p95 round" in markdown
    assert "### Process-local L1" in markdown
    assert "| `metadata_api` | 11 | 3 | 7 | 2 | 4 | 2 | 1 | 1 |" in markdown
    assert "### Valkey L2 client counters" in markdown
    assert "| `metadata_api` | 5 | 1 | 4 | 1 | 0 | 2 | 1 | 0 | 3 | 1 | 1 |" in markdown
    assert "### Redis/Valkey server counters" in markdown
    assert "| `metadata_api` | 5 | 10 | 0 | 2 |" in markdown
    assert "| `hot_path_cache` | 8 | 16 | 0 | 2 |" in markdown
    assert "| `steady_state_hot_path` | 9 | 18 | 0 | 2 |" in markdown
    assert "## Steady-state hot path windows" in markdown
    assert "| `cache-on` | 0 | 10.000 | 5.000 | 8.000 | 9.000 | 5 | 2 | 4 |" in markdown
    assert "Postgres + process-local L1" in markdown
    assert "## Percentile sample-size warnings" in markdown
    assert "## Benchmark interpretation notes" in markdown
