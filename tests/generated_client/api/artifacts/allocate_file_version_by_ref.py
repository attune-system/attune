from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.allocate_file_version_by_ref_request import (
    AllocateFileVersionByRefRequest,
)
from ...models.allocate_file_version_by_ref_response_201 import (
    AllocateFileVersionByRefResponse201,
)
from ...types import Response


def _get_kwargs(
    ref: str,
    *,
    body: AllocateFileVersionByRefRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/artifacts/ref/{ref}/versions/file".format(
            ref=quote(str(ref), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> AllocateFileVersionByRefResponse201 | Any | None:
    if response.status_code == 201:
        response_201 = AllocateFileVersionByRefResponse201.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[AllocateFileVersionByRefResponse201 | Any]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: AllocateFileVersionByRefRequest,
) -> Response[AllocateFileVersionByRefResponse201 | Any]:
    """Upsert an artifact by ref and allocate a file-backed version in one call.

     If the artifact doesn't exist, it is created using the supplied metadata.
    If it already exists, the execution link is updated (if provided).
    Then a new file-backed version is allocated and the `file_path` is returned.

    The caller writes the file to `$ATTUNE_ARTIFACTS_DIR/{file_path}` on the
    shared volume — no HTTP upload needed.

    Args:
        ref (str):
        body (AllocateFileVersionByRefRequest): Request DTO for the upsert-and-allocate endpoint.

            Looks up an artifact by ref (creating it if it doesn't exist), then
            allocates a new file-backed version and returns the `file_path` where
            the caller should write the file on the shared artifact volume.

            This replaces the multi-step create → 409-handling → allocate dance
            with a single API call.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AllocateFileVersionByRefResponse201 | Any]
    """

    kwargs = _get_kwargs(
        ref=ref,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: AllocateFileVersionByRefRequest,
) -> AllocateFileVersionByRefResponse201 | Any | None:
    """Upsert an artifact by ref and allocate a file-backed version in one call.

     If the artifact doesn't exist, it is created using the supplied metadata.
    If it already exists, the execution link is updated (if provided).
    Then a new file-backed version is allocated and the `file_path` is returned.

    The caller writes the file to `$ATTUNE_ARTIFACTS_DIR/{file_path}` on the
    shared volume — no HTTP upload needed.

    Args:
        ref (str):
        body (AllocateFileVersionByRefRequest): Request DTO for the upsert-and-allocate endpoint.

            Looks up an artifact by ref (creating it if it doesn't exist), then
            allocates a new file-backed version and returns the `file_path` where
            the caller should write the file on the shared artifact volume.

            This replaces the multi-step create → 409-handling → allocate dance
            with a single API call.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AllocateFileVersionByRefResponse201 | Any
    """

    return sync_detailed(
        ref=ref,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: AllocateFileVersionByRefRequest,
) -> Response[AllocateFileVersionByRefResponse201 | Any]:
    """Upsert an artifact by ref and allocate a file-backed version in one call.

     If the artifact doesn't exist, it is created using the supplied metadata.
    If it already exists, the execution link is updated (if provided).
    Then a new file-backed version is allocated and the `file_path` is returned.

    The caller writes the file to `$ATTUNE_ARTIFACTS_DIR/{file_path}` on the
    shared volume — no HTTP upload needed.

    Args:
        ref (str):
        body (AllocateFileVersionByRefRequest): Request DTO for the upsert-and-allocate endpoint.

            Looks up an artifact by ref (creating it if it doesn't exist), then
            allocates a new file-backed version and returns the `file_path` where
            the caller should write the file on the shared artifact volume.

            This replaces the multi-step create → 409-handling → allocate dance
            with a single API call.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AllocateFileVersionByRefResponse201 | Any]
    """

    kwargs = _get_kwargs(
        ref=ref,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: AllocateFileVersionByRefRequest,
) -> AllocateFileVersionByRefResponse201 | Any | None:
    """Upsert an artifact by ref and allocate a file-backed version in one call.

     If the artifact doesn't exist, it is created using the supplied metadata.
    If it already exists, the execution link is updated (if provided).
    Then a new file-backed version is allocated and the `file_path` is returned.

    The caller writes the file to `$ATTUNE_ARTIFACTS_DIR/{file_path}` on the
    shared volume — no HTTP upload needed.

    Args:
        ref (str):
        body (AllocateFileVersionByRefRequest): Request DTO for the upsert-and-allocate endpoint.

            Looks up an artifact by ref (creating it if it doesn't exist), then
            allocates a new file-backed version and returns the `file_path` where
            the caller should write the file on the shared artifact volume.

            This replaces the multi-step create → 409-handling → allocate dance
            with a single API call.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AllocateFileVersionByRefResponse201 | Any
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            body=body,
        )
    ).parsed
