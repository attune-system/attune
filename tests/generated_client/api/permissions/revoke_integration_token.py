from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.revoke_integration_token_request import RevokeIntegrationTokenRequest
from ...models.revoke_integration_token_response_200 import (
    RevokeIntegrationTokenResponse200,
)
from ...types import Response


def _get_kwargs(
    id: int,
    token_id: int,
    *,
    body: RevokeIntegrationTokenRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/identities/{id}/integration-tokens/{token_id}/revoke".format(
            id=quote(str(id), safe=""),
            token_id=quote(str(token_id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | RevokeIntegrationTokenResponse200 | None:
    if response.status_code == 200:
        response_200 = RevokeIntegrationTokenResponse200.from_dict(response.json())

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
) -> Response[Any | RevokeIntegrationTokenResponse200]:
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
    body: RevokeIntegrationTokenRequest,
) -> Response[Any | RevokeIntegrationTokenResponse200]:
    """
    Args:
        id (int):
        token_id (int):
        body (RevokeIntegrationTokenRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | RevokeIntegrationTokenResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        token_id=token_id,
        body=body,
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
    body: RevokeIntegrationTokenRequest,
) -> Any | RevokeIntegrationTokenResponse200 | None:
    """
    Args:
        id (int):
        token_id (int):
        body (RevokeIntegrationTokenRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | RevokeIntegrationTokenResponse200
    """

    return sync_detailed(
        id=id,
        token_id=token_id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: int,
    token_id: int,
    *,
    client: AuthenticatedClient,
    body: RevokeIntegrationTokenRequest,
) -> Response[Any | RevokeIntegrationTokenResponse200]:
    """
    Args:
        id (int):
        token_id (int):
        body (RevokeIntegrationTokenRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | RevokeIntegrationTokenResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        token_id=token_id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    token_id: int,
    *,
    client: AuthenticatedClient,
    body: RevokeIntegrationTokenRequest,
) -> Any | RevokeIntegrationTokenResponse200 | None:
    """
    Args:
        id (int):
        token_id (int):
        body (RevokeIntegrationTokenRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | RevokeIntegrationTokenResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            token_id=token_id,
            client=client,
            body=body,
        )
    ).parsed
