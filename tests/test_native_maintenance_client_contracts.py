"""Offline checks for the generated native-maintenance and analytics contracts."""

import json
from importlib import import_module
from pathlib import Path

import httpx
import pytest
from generated_client import AuthenticatedClient
from generated_client.api.retention import (
    get_native_maintenance_status,
    update_retention_config,
)
from generated_client.models.analytics_read_metadata import AnalyticsReadMetadata
from generated_client.models.api_response_retention_config import (
    ApiResponseRetentionConfig,
)
from generated_client.models.cache_retention_config import CacheRetentionConfig
from generated_client.models.dashboard_freshness_mode import DashboardFreshnessMode
from generated_client.models.maintenance_job import MaintenanceJob
from generated_client.models.managed_table import ManagedTable
from generated_client.models.native_maintenance_config import NativeMaintenanceConfig
from generated_client.models.native_maintenance_status import NativeMaintenanceStatus
from generated_client.models.retention_config import RetentionConfig
from generated_client.models.summary_kind import SummaryKind

pytestmark = pytest.mark.no_api


def test_generated_artifact_endpoint_is_packaged():
    endpoint = import_module("generated_client.api.artifacts.get_artifact")
    assert callable(endpoint.sync_detailed)
    assert callable(endpoint.asyncio_detailed)


def test_generated_native_defaults_match_the_exported_schema():
    spec = json.loads((Path(__file__).parents[1] / "web" / "openapi.json").read_text())
    properties = spec["components"]["schemas"]["NativeMaintenanceConfig"]["properties"]
    assert NativeMaintenanceConfig().to_dict() == {
        field: definition["default"] for field, definition in properties.items()
    }


def test_retention_update_round_trips_nested_statistics_without_dropping_native_settings():
    config = RetentionConfig(
        check_interval_seconds=1234,
        cache_retention=CacheRetentionConfig(
            statistics_interval_seconds=60,
            statistics_statement_timeout_milliseconds=2000,
            ddl_creation_statement_timeout_milliseconds=5000,
            ddl_statement_timeout_milliseconds=1000,
            ddl_lock_timeout_milliseconds=250,
        ),
        native_maintenance=NativeMaintenanceConfig(summary_interval_seconds=123),
    )
    body = config.to_dict()

    def respond(request):
        assert request.method == "PUT"
        assert request.url.path == "/api/v1/retention-config"
        assert request.headers["Authorization"] == "Bearer test-token"
        assert json.loads(request.content) == body
        return httpx.Response(200, json={"data": body})

    with AuthenticatedClient(
        base_url="http://operator.invalid", token="test-token",
        httpx_args={"transport": httpx.MockTransport(respond)},
    ) as client:
        result = update_retention_config.sync(client=client, body=config)
    assert isinstance(result, ApiResponseRetentionConfig)
    assert isinstance(result.data.cache_retention, CacheRetentionConfig)
    assert result.data.cache_retention.statistics_interval_seconds == 60
    assert result.data.cache_retention.statistics_statement_timeout_milliseconds == 2000
    assert result.data.to_dict() == body
    assert RetentionConfig.from_dict(body).to_dict() == body


def test_native_status_endpoint_parses_nested_typed_observations():
    body = {
        "enabled": False,
        "observed_at": "2026-10-06T12:00:00+00:00",
        "partitions": [{
            "parent": "event", "registered_partitions": 8, "future_partitions": 7,
            "missing_future_partitions": 3, "default_rows_at_least": 1001,
            "default_count_exact": False, "oldest_default_day": None,
        }],
        "summaries": [{
            "kind": "event_volume", "coverage_hours": 2,
            "covered_since": "2026-10-06T08:00:00+00:00",
            "covered_until": "2026-10-06T11:00:00+00:00",
            "dirty_hours": 1, "dirty_notifications": 4,
            "oldest_dirty_bucket": None, "oldest_notification": None,
            "latest_success": None,
        }],
        "schedule": [{
            "job": "summary", "next_due": "2026-10-06T12:05:00+00:00",
            "last_success": None,
        }],
    }

    def respond(request):
        assert request.method == "GET"
        assert request.url.path == "/api/v1/retention-config/native-status"
        assert request.headers["Authorization"] == "Bearer test-token"
        return httpx.Response(200, json={"data": body})

    with AuthenticatedClient(
        base_url="http://operator.invalid", token="test-token",
        httpx_args={"transport": httpx.MockTransport(respond)},
    ) as client:
        result = get_native_maintenance_status.sync(client=client)
    assert result is not None
    status = result.data
    assert status.partitions[0].parent == ManagedTable.EVENT
    assert status.summaries[0].kind == SummaryKind.EVENT_VOLUME
    assert status.schedule[0].job == MaintenanceJob.SUMMARY
    assert status.to_dict() == body
    assert NativeMaintenanceStatus.from_dict(body).to_dict() == body


def test_generated_analytics_coverage_preserves_holes_and_dirty_raw_ranges():
    metadata = {
        "mode": "summary_plus_raw",
        "summary_ranges": [
            {"start": "2026-10-06T08:00:00+00:00", "end": "2026-10-06T09:00:00+00:00"},
            {"start": "2026-10-06T10:00:00+00:00", "end": "2026-10-06T11:00:00+00:00"},
        ],
        "raw_ranges": [
            {"start": "2026-10-06T09:00:00+00:00", "end": "2026-10-06T10:00:00+00:00"},
        ],
        "oldest_refresh": None,
    }
    parsed = AnalyticsReadMetadata.from_dict(metadata)
    assert parsed.mode == DashboardFreshnessMode.SUMMARY_PLUS_RAW
    assert len(parsed.summary_ranges) == 2
    assert len(parsed.raw_ranges) == 1
    assert parsed.to_dict() == metadata
