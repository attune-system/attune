from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.save_workflow_file_request import SaveWorkflowFileRequest
from ...models.save_workflow_file_response_201 import SaveWorkflowFileResponse201
from ...types import Response


def _get_kwargs(
    pack_ref: str,
    *,
    body: SaveWorkflowFileRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/packs/{pack_ref}/workflow-files".format(
            pack_ref=quote(str(pack_ref), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | SaveWorkflowFileResponse201 | None:
    if response.status_code == 201:
        response_201 = SaveWorkflowFileResponse201.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 409:
        response_409 = cast(Any, None)
        return response_409

    if response.status_code == 500:
        response_500 = cast(Any, None)
        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | SaveWorkflowFileResponse201]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    pack_ref: str,
    *,
    client: AuthenticatedClient,
    body: SaveWorkflowFileRequest,
) -> Response[Any | SaveWorkflowFileResponse201]:
    """Save a workflow file to disk and sync it to the database

     Writes a `{name}.workflow.yaml` file to `{packs_base_dir}/{pack_ref}/actions/workflows/`
    and creates or updates the corresponding workflow_definition record in the database.
    Also creates a companion action record so the workflow appears in action lists and palettes.

    Args:
        pack_ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | SaveWorkflowFileResponse201]
    """

    kwargs = _get_kwargs(
        pack_ref=pack_ref,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    pack_ref: str,
    *,
    client: AuthenticatedClient,
    body: SaveWorkflowFileRequest,
) -> Any | SaveWorkflowFileResponse201 | None:
    """Save a workflow file to disk and sync it to the database

     Writes a `{name}.workflow.yaml` file to `{packs_base_dir}/{pack_ref}/actions/workflows/`
    and creates or updates the corresponding workflow_definition record in the database.
    Also creates a companion action record so the workflow appears in action lists and palettes.

    Args:
        pack_ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | SaveWorkflowFileResponse201
    """

    return sync_detailed(
        pack_ref=pack_ref,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    pack_ref: str,
    *,
    client: AuthenticatedClient,
    body: SaveWorkflowFileRequest,
) -> Response[Any | SaveWorkflowFileResponse201]:
    """Save a workflow file to disk and sync it to the database

     Writes a `{name}.workflow.yaml` file to `{packs_base_dir}/{pack_ref}/actions/workflows/`
    and creates or updates the corresponding workflow_definition record in the database.
    Also creates a companion action record so the workflow appears in action lists and palettes.

    Args:
        pack_ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | SaveWorkflowFileResponse201]
    """

    kwargs = _get_kwargs(
        pack_ref=pack_ref,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    pack_ref: str,
    *,
    client: AuthenticatedClient,
    body: SaveWorkflowFileRequest,
) -> Any | SaveWorkflowFileResponse201 | None:
    """Save a workflow file to disk and sync it to the database

     Writes a `{name}.workflow.yaml` file to `{packs_base_dir}/{pack_ref}/actions/workflows/`
    and creates or updates the corresponding workflow_definition record in the database.
    Also creates a companion action record so the workflow appears in action lists and palettes.

    Args:
        pack_ref (str):
        body (SaveWorkflowFileRequest): Request DTO for saving a workflow file to disk and syncing
            to DB

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | SaveWorkflowFileResponse201
    """

    return (
        await asyncio_detailed(
            pack_ref=pack_ref,
            client=client,
            body=body,
        )
    ).parsed
