from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.auth_error_response import AuthErrorResponse
from ...models.error_response import ErrorResponse
from ...models.update_pack_index_response_200 import UpdatePackIndexResponse200
from ...models.update_pack_registry_index_request import UpdatePackRegistryIndexRequest
from ...types import Response


def _get_kwargs(
    id: int,
    *,
    body: UpdatePackRegistryIndexRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/pack-indices/{id}".format(
            id=quote(str(id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200 | None:
    if response.status_code == 200:
        response_200 = UpdatePackIndexResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = AuthErrorResponse.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = ErrorResponse.from_dict(response.json())

        return response_403

    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404

    if response.status_code == 409:
        response_409 = ErrorResponse.from_dict(response.json())

        return response_409

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200]:
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
    body: UpdatePackRegistryIndexRequest,
) -> Response[AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200]:
    """
    Args:
        id (int):
        body (UpdatePackRegistryIndexRequest): Request to update a configured pack registry index.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200]
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
    body: UpdatePackRegistryIndexRequest,
) -> AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200 | None:
    """
    Args:
        id (int):
        body (UpdatePackRegistryIndexRequest): Request to update a configured pack registry index.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200
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
    body: UpdatePackRegistryIndexRequest,
) -> Response[AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200]:
    """
    Args:
        id (int):
        body (UpdatePackRegistryIndexRequest): Request to update a configured pack registry index.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200]
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
    body: UpdatePackRegistryIndexRequest,
) -> AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200 | None:
    """
    Args:
        id (int):
        body (UpdatePackRegistryIndexRequest): Request to update a configured pack registry index.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | ErrorResponse | UpdatePackIndexResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
