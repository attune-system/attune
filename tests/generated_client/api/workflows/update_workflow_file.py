from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.save_workflow_file_request import SaveWorkflowFileRequest
from ...models.update_workflow_file_response_200 import UpdateWorkflowFileResponse200
from ...types import Response


def _get_kwargs(
    ref: str,
    *,
    body: SaveWorkflowFileRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/workflows/{ref}/file".format(
            ref=quote(str(ref), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | UpdateWorkflowFileResponse200 | None:
    if response.status_code == 200:
        response_200 = UpdateWorkflowFileResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 500:
        response_500 = cast(Any, None)
        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | UpdateWorkflowFileResponse200]:
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
    body: SaveWorkflowFileRequest,
) -> Response[Any | UpdateWorkflowFileResponse200]:
    """Update a workflow file on disk and sync changes to the database

    Args:
        ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UpdateWorkflowFileResponse200]
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
    body: SaveWorkflowFileRequest,
) -> Any | UpdateWorkflowFileResponse200 | None:
    """Update a workflow file on disk and sync changes to the database

    Args:
        ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UpdateWorkflowFileResponse200
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
    body: SaveWorkflowFileRequest,
) -> Response[Any | UpdateWorkflowFileResponse200]:
    """Update a workflow file on disk and sync changes to the database

    Args:
        ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UpdateWorkflowFileResponse200]
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
    body: SaveWorkflowFileRequest,
) -> Any | UpdateWorkflowFileResponse200 | None:
    """Update a workflow file on disk and sync changes to the database

    Args:
        ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UpdateWorkflowFileResponse200
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            body=body,
        )
    ).parsed
