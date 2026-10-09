"""Offline generated-client checks for cache refresh coordination."""

import json
from pathlib import Path

import httpx
import pytest
from generated_client import AuthenticatedClient
from generated_client.api.caches import create_generation
from generated_client.models.cache_generation_api_response import (
    CacheGenerationApiResponse,
)
from generated_client.models.cache_generation_response import CacheGenerationResponse
from generated_client.models.cache_refresh_concurrency import CacheRefreshConcurrency
from generated_client.models.cache_retention_config import CacheRetentionConfig
from generated_client.models.create_cache_generation_request import (
    CreateCacheGenerationRequest,
)
from generated_client.models.create_cache_namespace_request import (
    CreateCacheNamespaceRequest,
)
from generated_client.models.error_response import ErrorResponse
from generated_client.types import UNSET

pytestmark = pytest.mark.no_api


def test_partition_creation_deadline_is_a_typed_independent_field():
    body = {
        "ddl_creation_statement_timeout_milliseconds": 5000,
        "ddl_statement_timeout_milliseconds": 1000,
        "ddl_lock_timeout_milliseconds": 250,
    }
    config = CacheRetentionConfig.from_dict(body)
    assert config.ddl_creation_statement_timeout_milliseconds == 5000
    assert config.ddl_statement_timeout_milliseconds == 1000
    assert config.ddl_lock_timeout_milliseconds == 250
    assert not config.additional_properties
    assert config.to_dict() == body


@pytest.mark.parametrize("interval, deadline", [(1, 1), (300, 5000), (86400, 3600000)])
def test_statistics_settings_are_typed_independently_of_ddl_deadlines(interval, deadline):
    config = CacheRetentionConfig(
        statistics_interval_seconds=interval,
        statistics_statement_timeout_milliseconds=deadline,
        ddl_creation_statement_timeout_milliseconds=5000,
        ddl_statement_timeout_milliseconds=1000,
        ddl_lock_timeout_milliseconds=250,
    )
    body = config.to_dict()
    parsed = CacheRetentionConfig.from_dict(body)
    assert parsed.statistics_interval_seconds == interval
    assert parsed.statistics_statement_timeout_milliseconds == deadline
    assert parsed.ddl_creation_statement_timeout_milliseconds == 5000
    assert parsed.ddl_statement_timeout_milliseconds == 1000
    assert parsed.ddl_lock_timeout_milliseconds == 250
    assert not parsed.additional_properties
    assert parsed.to_dict() == body


def test_omitted_statistics_settings_stay_omitted_for_server_defaults():
    config = CacheRetentionConfig.from_dict({})
    assert config.statistics_interval_seconds is UNSET
    assert config.statistics_statement_timeout_milliseconds is UNSET
    assert config.to_dict() == {}


def generation_body(execution_id):
    return {
        "generation_id": 42, "namespace_id": 7, "status": "staging",
        "client_refresh_id": "original", "expected_active_generation_id": None,
        "expected_chunk_count": 0, "expected_record_count": None,
        "expected_size_bytes": None, "record_count": 0, "size_bytes": 0,
        "checksum_algorithm": None, "checksum": None, "source_revision": None,
        "created_by": 5, "created_by_execution": execution_id,
        "created": "2026-10-07T12:00:00+00:00", "sealed": None,
        "activated": None, "retired": None, "readable_until": None,
        "failed": None, "failure_reason": None,
    }


@pytest.mark.parametrize("policy", ["reuse", "conflict", "parallel"])
def test_refresh_policy_is_typed_and_remains_flat(policy):
    request = CreateCacheNamespaceRequest.from_dict({
        "owner_type": "pack", "owner_ref": "example", "namespace": "users",
        "refresh_concurrency": policy,
    })
    assert request.refresh_concurrency == CacheRefreshConcurrency(policy)
    assert request.to_dict()["refresh_concurrency"] == policy
    assert "policy" not in request.to_dict()


@pytest.mark.parametrize("execution_id", [None, 123])
def test_execution_attribution_round_trips_as_a_typed_nullable_field(execution_id):
    body = generation_body(execution_id)
    generation = CacheGenerationResponse.from_dict(body)
    assert generation.created_by == 5
    assert generation.created_by_execution == execution_id
    assert generation.to_dict() == body


@pytest.mark.parametrize("status", [200, 201, 409])
def test_begin_parses_created_reused_and_conflicting_generations(status):
    def respond(request):
        assert request.method == "POST"
        assert request.url.path == "/api/v1/cache/namespaces/users/generations"
        assert request.headers["Authorization"] == "Bearer test-token"
        assert "created_by_execution" not in json.loads(request.content)
        if status == 409:
            return httpx.Response(status, json={
                "error": "cache namespace already has an unpublished generation",
                "code": "cache_refresh_in_progress",
                "details": {"generation_id": 42, "created_by_execution": 123},
            })
        return httpx.Response(status, json={"data": generation_body(123)})

    body = CreateCacheGenerationRequest.from_dict({
        "owner_type": "pack", "owner_ref": "example", "client_refresh_id": "requested",
        "expected_active_generation_id": None, "expected_chunk_count": 0,
    })
    with AuthenticatedClient(
        base_url="http://cache.invalid", token="test-token",
        httpx_args={"transport": httpx.MockTransport(respond)},
    ) as client:
        result = create_generation.sync_detailed("users", client=client, body=body)
    assert result.status_code == status
    if status == 409:
        assert isinstance(result.parsed, ErrorResponse)
        assert result.parsed.code == "cache_refresh_in_progress"
        assert result.parsed.details == {"generation_id": 42, "created_by_execution": 123}
    else:
        assert isinstance(result.parsed, CacheGenerationApiResponse)
        assert result.parsed.data.client_refresh_id == "original"
        assert result.parsed.data.created_by_execution == 123


def test_openapi_keeps_execution_attribution_server_only():
    spec = json.loads((Path(__file__).parents[1] / "web" / "openapi.json").read_text())
    schemas = spec["components"]["schemas"]
    assert set(schemas["CacheRefreshConcurrency"]["enum"]) == {"reuse", "conflict", "parallel"}
    request = schemas["CreateCacheGenerationRequest"]
    assert "created_by_execution" not in request["properties"]
    assert request["additionalProperties"] is False
    assert "created_by_execution" in schemas["CacheGenerationResponse"]["required"]


def test_openapi_cache_statistics_settings_match_generated_fields():
    spec = json.loads((Path(__file__).parents[1] / "web" / "openapi.json").read_text())
    schema = spec["components"]["schemas"]["CacheRetentionConfig"]
    generated_fields = {
        field.name for field in CacheRetentionConfig.__attrs_attrs__
        if field.name != "additional_properties"
    }
    assert generated_fields == set(schema["properties"])
    for field in ["statistics_interval_seconds", "statistics_statement_timeout_milliseconds"]:
        assert schema["properties"][field]["type"] == "integer"
        assert field not in schema.get("required", [])
