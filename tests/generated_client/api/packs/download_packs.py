from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_download_packs_response import (
    ApiResponseDownloadPacksResponse,
)
from ...models.download_packs_request import DownloadPacksRequest
from ...types import Response


def _get_kwargs(
    *,
    body: DownloadPacksRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/packs/download",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseDownloadPacksResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseDownloadPacksResponse.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ApiResponseDownloadPacksResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: DownloadPacksRequest,
) -> Response[Any | ApiResponseDownloadPacksResponse]:
    """Create pack routes

     Note: Nested resource routes (e.g., /packs/:ref/actions) are defined
    in their respective modules (actions.rs, triggers.rs, rules.rs) to avoid
    route conflicts and maintain proper separation of concerns.
    Download packs from various sources

    Args:
        body (DownloadPacksRequest): Request DTO for downloading packs

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseDownloadPacksResponse]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    body: DownloadPacksRequest,
) -> Any | ApiResponseDownloadPacksResponse | None:
    """Create pack routes

     Note: Nested resource routes (e.g., /packs/:ref/actions) are defined
    in their respective modules (actions.rs, triggers.rs, rules.rs) to avoid
    route conflicts and maintain proper separation of concerns.
    Download packs from various sources

    Args:
        body (DownloadPacksRequest): Request DTO for downloading packs

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseDownloadPacksResponse
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: DownloadPacksRequest,
) -> Response[Any | ApiResponseDownloadPacksResponse]:
    """Create pack routes

     Note: Nested resource routes (e.g., /packs/:ref/actions) are defined
    in their respective modules (actions.rs, triggers.rs, rules.rs) to avoid
    route conflicts and maintain proper separation of concerns.
    Download packs from various sources

    Args:
        body (DownloadPacksRequest): Request DTO for downloading packs

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseDownloadPacksResponse]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: DownloadPacksRequest,
) -> Any | ApiResponseDownloadPacksResponse | None:
    """Create pack routes

     Note: Nested resource routes (e.g., /packs/:ref/actions) are defined
    in their respective modules (actions.rs, triggers.rs, rules.rs) to avoid
    route conflicts and maintain proper separation of concerns.
    Download packs from various sources

    Args:
        body (DownloadPacksRequest): Request DTO for downloading packs

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseDownloadPacksResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
