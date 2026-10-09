from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.create_file_version_request import CreateFileVersionRequest
from ...models.create_version_file_response_201 import CreateVersionFileResponse201
from ...types import Response


def _get_kwargs(
    id: int,
    *,
    body: CreateFileVersionRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/artifacts/{id}/versions/file".format(
            id=quote(str(id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | CreateVersionFileResponse201 | None:
    if response.status_code == 201:
        response_201 = CreateVersionFileResponse201.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | CreateVersionFileResponse201]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    body: CreateFileVersionRequest,
) -> Response[Any | CreateVersionFileResponse201]:
    """Create a new file-backed version (no file content in request).

     This endpoint allocates a version number and computes a `file_path` on the
    shared artifact volume. The caller (execution process) is expected to write
    the file content directly to `$ATTUNE_ARTIFACTS_DIR/{file_path}` after
    receiving the response. The worker finalizes `size_bytes` after execution.

    Only applicable to file-type artifacts (FileBinary, FileDatatable, FileText, Log).

    Args:
        id (int):
        body (CreateFileVersionRequest): Request DTO for creating a new file-backed artifact
            version.
            No file content is included — the caller writes the file directly to
            `$ATTUNE_ARTIFACTS_DIR/{file_path}` after receiving the response.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | CreateVersionFileResponse201]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    *,
    client: AuthenticatedClient,
    body: CreateFileVersionRequest,
) -> Any | CreateVersionFileResponse201 | None:
    """Create a new file-backed version (no file content in request).

     This endpoint allocates a version number and computes a `file_path` on the
    shared artifact volume. The caller (execution process) is expected to write
    the file content directly to `$ATTUNE_ARTIFACTS_DIR/{file_path}` after
    receiving the response. The worker finalizes `size_bytes` after execution.

    Only applicable to file-type artifacts (FileBinary, FileDatatable, FileText, Log).

    Args:
        id (int):
        body (CreateFileVersionRequest): Request DTO for creating a new file-backed artifact
            version.
            No file content is included — the caller writes the file directly to
            `$ATTUNE_ARTIFACTS_DIR/{file_path}` after receiving the response.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | CreateVersionFileResponse201
    """

    return sync_detailed(
        id=id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    body: CreateFileVersionRequest,
) -> Response[Any | CreateVersionFileResponse201]:
    """Create a new file-backed version (no file content in request).

     This endpoint allocates a version number and computes a `file_path` on the
    shared artifact volume. The caller (execution process) is expected to write
    the file content directly to `$ATTUNE_ARTIFACTS_DIR/{file_path}` after
    receiving the response. The worker finalizes `size_bytes` after execution.

    Only applicable to file-type artifacts (FileBinary, FileDatatable, FileText, Log).

    Args:
        id (int):
        body (CreateFileVersionRequest): Request DTO for creating a new file-backed artifact
            version.
            No file content is included — the caller writes the file directly to
            `$ATTUNE_ARTIFACTS_DIR/{file_path}` after receiving the response.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | CreateVersionFileResponse201]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    *,
    client: AuthenticatedClient,
    body: CreateFileVersionRequest,
) -> Any | CreateVersionFileResponse201 | None:
    """Create a new file-backed version (no file content in request).

     This endpoint allocates a version number and computes a `file_path` on the
    shared artifact volume. The caller (execution process) is expected to write
    the file content directly to `$ATTUNE_ARTIFACTS_DIR/{file_path}` after
    receiving the response. The worker finalizes `size_bytes` after execution.

    Only applicable to file-type artifacts (FileBinary, FileDatatable, FileText, Log).

    Args:
        id (int):
        body (CreateFileVersionRequest): Request DTO for creating a new file-backed artifact
            version.
            No file content is included — the caller writes the file directly to
            `$ATTUNE_ARTIFACTS_DIR/{file_path}` after receiving the response.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | CreateVersionFileResponse201
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
