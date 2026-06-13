from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.delete_integration_token_response_200 import (
    DeleteIntegrationTokenResponse200,
)
from ...types import Response


def _get_kwargs(
    id: int,
    token_id: int,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/api/v1/identities/{id}/integration-tokens/{token_id}".format(
            id=quote(str(id), safe=""),
            token_id=quote(str(token_id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | DeleteIntegrationTokenResponse200 | None:
    if response.status_code == 200:
        response_200 = DeleteIntegrationTokenResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | DeleteIntegrationTokenResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: int,
    token_id: int,
    *,
    client: AuthenticatedClient,
) -> Response[Any | DeleteIntegrationTokenResponse200]:
    """
    Args:
        id (int):
        token_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | DeleteIntegrationTokenResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        token_id=token_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    token_id: int,
    *,
    client: AuthenticatedClient,
) -> Any | DeleteIntegrationTokenResponse200 | None:
    """
    Args:
        id (int):
        token_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | DeleteIntegrationTokenResponse200
    """

    return sync_detailed(
        id=id,
        token_id=token_id,
        client=client,
    ).parsed


async def asyncio_detailed(
    id: int,
    token_id: int,
    *,
    client: AuthenticatedClient,
) -> Response[Any | DeleteIntegrationTokenResponse200]:
    """
    Args:
        id (int):
        token_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | DeleteIntegrationTokenResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        token_id=token_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    token_id: int,
    *,
    client: AuthenticatedClient,
) -> Any | DeleteIntegrationTokenResponse200 | None:
    """
    Args:
        id (int):
        token_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | DeleteIntegrationTokenResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            token_id=token_id,
            client=client,
        )
    ).parsed
