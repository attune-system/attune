from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_build_pack_envs_response import (
    ApiResponseBuildPackEnvsResponse,
)
from ...models.build_pack_envs_request import BuildPackEnvsRequest
from ...types import Response


def _get_kwargs(
    *,
    body: BuildPackEnvsRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/packs/build-envs",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseBuildPackEnvsResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseBuildPackEnvsResponse.from_dict(response.json())

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
) -> Response[Any | ApiResponseBuildPackEnvsResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: BuildPackEnvsRequest,
) -> Response[Any | ApiResponseBuildPackEnvsResponse]:
    """Build pack environments

    Args:
        body (BuildPackEnvsRequest): Request DTO for building pack environments

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseBuildPackEnvsResponse]
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
    body: BuildPackEnvsRequest,
) -> Any | ApiResponseBuildPackEnvsResponse | None:
    """Build pack environments

    Args:
        body (BuildPackEnvsRequest): Request DTO for building pack environments

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseBuildPackEnvsResponse
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: BuildPackEnvsRequest,
) -> Response[Any | ApiResponseBuildPackEnvsResponse]:
    """Build pack environments

    Args:
        body (BuildPackEnvsRequest): Request DTO for building pack environments

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseBuildPackEnvsResponse]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: BuildPackEnvsRequest,
) -> Any | ApiResponseBuildPackEnvsResponse | None:
    """Build pack environments

    Args:
        body (BuildPackEnvsRequest): Request DTO for building pack environments

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseBuildPackEnvsResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
