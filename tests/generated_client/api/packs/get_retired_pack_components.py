from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.auth_error_response import AuthErrorResponse
from ...models.error_response import ErrorResponse
from ...models.get_retired_pack_components_response_200 import (
    GetRetiredPackComponentsResponse200,
)
from ...types import Response


def _get_kwargs(
    ref: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/packs/{ref}/retired-components".format(
            ref=quote(str(ref), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200 | None:
    if response.status_code == 200:
        response_200 = GetRetiredPackComponentsResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = AuthErrorResponse.from_dict(response.json())

        return response_401

    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200]:
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
) -> Response[AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200]:
    """
    Args:
        ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200]
    """

    kwargs = _get_kwargs(
        ref=ref,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient,
) -> AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200 | None:
    """
    Args:
        ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200
    """

    return sync_detailed(
        ref=ref,
        client=client,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
) -> Response[AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200]:
    """
    Args:
        ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200]
    """

    kwargs = _get_kwargs(
        ref=ref,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient,
) -> AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200 | None:
    """
    Args:
        ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | ErrorResponse | GetRetiredPackComponentsResponse200
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
        )
    ).parsed
