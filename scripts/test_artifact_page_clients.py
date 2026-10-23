"""Offline generated-client pagination contracts; uses the E2E venv's httpx/attrs."""

from pathlib import Path
import sys
import unittest

import httpx

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tests"))
from generated_client import AuthenticatedClient
from generated_client.api.artifacts import list_artifacts_by_execution, list_versions
from generated_client.models.artifact_type import ArtifactType
from generated_client.models.paginated_response_artifact_summary import (
    PaginatedResponseArtifactSummary,
)
from generated_client.models.paginated_response_artifact_version_summary import (
    PaginatedResponseArtifactVersionSummary,
)


class ArtifactPageClientTests(unittest.TestCase):
    def test_execution_page_and_progress_filter_use_the_typed_paginated_contract(self):
        def respond(request):
            self.assertEqual(request.url.path, "/api/v1/executions/42/artifacts")
            self.assertEqual(
                dict(request.url.params),
                {"page": "1", "per_page": "1", "type": "progress"},
            )
            return httpx.Response(
                200,
                json={
                    "items": [
                        {
                            "id": 17,
                            "ref": "fixture.progress",
                            "scope": "action",
                            "owner": "fixture.run",
                            "type": "progress",
                            "visibility": "public",
                            "classification": "general",
                            "created": "2026-10-14T00:00:00Z",
                            "updated": "2026-10-14T00:00:00Z",
                        }
                    ],
                    "pagination": {
                        "page": 1,
                        "page_size": 1,
                        "total_items": 2,
                        "total_pages": 2,
                        "has_previous": False,
                        "has_next": True,
                    },
                },
            )

        with AuthenticatedClient(
            base_url="http://artifact-contract.invalid",
            token="fixture-token",
            httpx_args={"transport": httpx.MockTransport(respond)},
        ) as client:
            result = list_artifacts_by_execution.sync(
                42, client=client, page=1, per_page=1, type_=ArtifactType.PROGRESS
            )
        self.assertIsInstance(result, PaginatedResponseArtifactSummary)
        self.assertEqual(result.items[0].id, 17)
        self.assertEqual(result.pagination.total_items, 2)

    def test_history_page_retains_navigation_metadata(self):
        def respond(request):
            self.assertEqual(request.url.path, "/api/v1/artifacts/17/versions")
            self.assertEqual(dict(request.url.params), {"page": "2", "per_page": "100"})
            return httpx.Response(
                200,
                json={
                    "items": [
                        {"id": 3, "version": 1, "created": "2026-10-14T00:00:00Z"}
                    ],
                    "pagination": {
                        "page": 2,
                        "page_size": 100,
                        "total_items": 101,
                        "total_pages": 2,
                        "has_previous": True,
                        "has_next": False,
                    },
                },
            )

        with AuthenticatedClient(
            base_url="http://artifact-contract.invalid",
            token="fixture-token",
            httpx_args={"transport": httpx.MockTransport(respond)},
        ) as client:
            result = list_versions.sync(17, client=client, page=2, per_page=100)
        self.assertIsInstance(result, PaginatedResponseArtifactVersionSummary)
        self.assertEqual(result.items[0].version, 1)
        self.assertEqual(result.pagination.total_items, 101)
        self.assertFalse(result.pagination.has_next)

    def test_invalid_pages_do_not_parse_as_successful_empty_histories(self):
        with AuthenticatedClient(
            base_url="http://artifact-contract.invalid",
            token="fixture-token",
            httpx_args={
                "transport": httpx.MockTransport(lambda _: httpx.Response(400))
            },
        ) as client:
            response = list_versions.sync_detailed(17, client=client, page=0)
        self.assertEqual(response.status_code, 400)
        self.assertIsNone(response.parsed)

    def test_unbounded_data_arrays_are_not_an_alternate_v1_reader(self):
        for model in [
            PaginatedResponseArtifactSummary,
            PaginatedResponseArtifactVersionSummary,
        ]:
            with self.assertRaises(KeyError):
                model.from_dict({"data": []})


if __name__ == "__main__":
    unittest.main()
